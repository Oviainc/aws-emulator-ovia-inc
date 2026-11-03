// SPDX-License-Identifier: CC0-1.0

//! What a Step Functions `Task` state whose work is a LAMBDA FUNCTION does, read where
//! a client reads it: the `AWS/Lambda` series CloudWatch answers for that function.
//!
//! RULE: a `Task` state whose work is a Lambda function performs a real invocation of
//! that function, acting as the state machine's execution role — so the role's
//! `lambda:InvokeFunction` grant on the function's ARN decides whether the invocation
//! happens at all — and Lambda meters every invocation it admits as one `Invocations`
//! datapoint of that function's own series. A task the role may not make is refused
//! before the request goes anywhere: the execution fails with Lambda's own
//! access-denied refusal and the function's series stays untouched. The `Resource`
//! form does not change any of that: a bare function ARN names the function itself,
//! and the optimized (`arn:aws:states:::lambda:invoke`) and AWS SDK
//! (`arn:aws:states:::aws-sdk:lambda:invoke`) integrations name the same invocation
//! through a `FunctionName` parameter.
//!
//! Four crates have to agree for that to hold — Step Functions decides and makes the
//! invocation, IAM holds the documents it is judged against, Lambda serves it and
//! publishes the series, and CloudWatch answers for the series — so the suite lives
//! here rather than in any of them.
//!
//! The rows differ along what the rule generalises over: which `Resource` form names
//! the work (the function's own ARN, the optimized integration, the AWS SDK
//! integration), the form the request addresses the function in (its ARN, its bare
//! name), whether the execution role grants the invocation, and whether the function
//! the work names exists at all.
//!
//! What this suite does NOT assert is the RESULT of the run: running a deployment
//! package is not implemented in this emulator (`Invoke` says so and returns no
//! payload the function did not produce), so no row here claims a function's output.
//! What is asserted is the invocation itself — the datapoint Lambda publishes for an
//! invocation it admitted, and the refusal of one it never made.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder for an `application/x-www-form-urlencoded` component and the
// reader of one element of an awsQuery answer: `tests/all.rs` is one crate root, so
// this suite borrows both rather than carrying a second copy.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;
use crate::query_wire::element;

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The namespace Lambda publishes its own metrics to, and the names the AWS Lambda
/// metric reference gives the invocation count, its unit and the dimensions it is
/// reported per. Written as literals here rather than read from the Lambda crate, so a
/// series published under a name no AWS reader would ask for fails here.
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const INVOCATIONS_SERIES: &str = "Invocations";
/// See [`NAMESPACE`].
const COUNT_UNIT: &str = "Count";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";
/// See [`NAMESPACE`].
const RESOURCE_DIMENSION: &str = "Resource";

/// The `Period`, in seconds, every query reads at: the one-minute granularity Lambda's
/// own series are stored at.
const PERIOD_SECS: i64 = 60;

/// How far either side of now a query's window reaches, in seconds — wide enough that
/// the minute an invocation was published in is inside it however a row straddles a
/// minute boundary.
const WINDOW_SECS: f64 = 600.0;

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a function reports once it can be invoked
/// (`FunctionConfiguration$State`: "Active" once "the function is ready to be
/// invoked").
const FUNCTION_ACTIVE: &str = "Active";

/// An opaque deployment package, base64-encoded as `FunctionCode$ZipFile` is on the
/// wire. Its CONTENTS are never interpreted here: no row claims a function ran.
const PACKAGE_ZIP_BASE64: &str = "cGFja2FnZQ==";

/// How long a row waits for a documented asynchronous transition — an execution
/// leaving `RUNNING`, a function reaching [`FUNCTION_ACTIVE`].
const PATIENCE: Duration = Duration::from_secs(20);

/// How long one poll sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The error name a `Task` state reports for a Lambda refusal the `Invoke` operation
/// does not model, which is what an access denial arrives as.
const LAMBDA_SDK_REFUSAL: &str = "Lambda.AWSLambdaException";

/// The error name a `Task` state reports when Lambda itself refuses the invocation
/// because the function does not exist (`Invoke`'s modelled
/// `ResourceNotFoundException`).
const LAMBDA_NOT_FOUND: &str = "Lambda.ResourceNotFoundException";

/// The four services this question spans, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: this router runs with request
/// authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One call to an `awsJson` service, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Step Functions call, requiring a 200.
async fn sfn(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "states", "AWSStepFunctions", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One CloudWatch call, requiring a 200.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = rpc(
        router,
        "monitoring",
        "GraniteServiceVersion20100801",
        "1.0",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One `restJson1` call to Lambda, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One awsQuery call to IAM, requiring a 200, answered with the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected 200 from IAM, got {status} with {text}"
    );
    text
}

/// The text a member of a response carries.
fn string(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a {name} in {body}"))
        .to_string()
}

/// Now, in epoch seconds, which is how CloudWatch's `awsJson` wire form carries an
/// instant.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// An IAM role `service` may assume, carrying `policy` as its inline policy; answers
/// its ARN.
///
/// A state machine's execution role is trusted by Step Functions and a function's own
/// execution role by Lambda — which Lambda's `CreateFunction` checks ("The role defined
/// for the function cannot be assumed by Lambda") — so the trusted service is the
/// caller's.
async fn role(
    router: &MultiServiceRouter,
    name: &str,
    service: &str,
    policy: &Value,
) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service },
            "Action": "sts:AssumeRole",
        }],
    });
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust.to_string()),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "inline".to_string()),
            ("PolicyDocument", policy.to_string()),
        ],
    )
    .await;
    arn
}

/// The service a state machine's execution role is assumed by, and the one a
/// function's own execution role is.
const STATES_PRINCIPAL: &str = "states.amazonaws.com";
/// See [`STATES_PRINCIPAL`].
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// A function waited out to [`FUNCTION_ACTIVE`], so an invocation of it is one Lambda
/// admits; answers its ARN.
async fn given_function(router: &MultiServiceRouter, name: &str, role_arn: &str) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": PACKAGE_ZIP_BASE64 },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateFunction must answer 201, got {status} with {created}"
    );
    let arn = string(&created, "FunctionArn");
    let began = Instant::now();
    loop {
        let (status, configuration) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetFunctionConfiguration must resolve the function the create accepted, got \
             {status} with {configuration}"
        );
        if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
            return arn;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "the function must reach {FUNCTION_ACTIVE} within {PATIENCE:?}, last saw \
             {configuration}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A state machine whose one `Task` state does `resource`'s work with `parameters` (and
/// with the execution's own input when a row names none), under `role_arn`; answers its
/// ARN.
async fn machine(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    resource: &str,
    parameters: Option<&Value>,
    role_arn: &str,
) -> String {
    let mut task = json!({ "Type": "Task", "Resource": resource, "End": true });
    if let Some(parameters) = parameters {
        task["Parameters"] = parameters.clone();
    }
    let definition = json!({ "StartAt": "Work", "States": { "Work": task } });
    let created = sfn(
        router,
        case,
        "CreateStateMachine",
        &json!({
            "name": name,
            "definition": definition.to_string(),
            "roleArn": role_arn,
        }),
    )
    .await;
    string(&created, "stateMachineArn")
}

/// Start one execution with `input` and report it once it has left `RUNNING`.
async fn ran(router: &MultiServiceRouter, case: &str, machine_arn: &str, input: &Value) -> Value {
    let started = sfn(
        router,
        case,
        "StartExecution",
        &json!({
            "stateMachineArn": machine_arn,
            "name": "only",
            "input": input.to_string(),
        }),
    )
    .await;
    let execution_arn = string(&started, "executionArn");
    let began = Instant::now();
    loop {
        let described = sfn(
            router,
            case,
            "DescribeExecution",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "case {case:?}: expected a terminal status within {PATIENCE:?}, got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The invocations the `Invocations` series the dimensions `dimensions` identify
/// reports over the window: the SUM, which is the magnitude a client charts or alarms
/// on. A series with no datapoint at all sums to zero, which is what "the function's
/// series stayed empty" reads as.
async fn invocations(router: &MultiServiceRouter, case: &str, dimensions: &Value) -> f64 {
    let now = now_secs();
    let stats = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": INVOCATIONS_SERIES,
            "Dimensions": dimensions,
            "StartTime": now - WINDOW_SECS,
            "EndTime": now + WINDOW_SECS,
            "Period": PERIOD_SECS,
            "Statistics": ["Sum"],
            "Unit": COUNT_UNIT,
        }),
    )
    .await;
    stats["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// The dimension set of the series reported per function.
fn function_dimensions(function: &str) -> Value {
    json!([{ "Name": FUNCTION_DIMENSION, "Value": function }])
}

/// The dimension set of the series reported per invoked resource, which for an
/// invocation that named no qualifier is the function itself.
fn resource_dimensions(function: &str) -> Value {
    json!([
        { "Name": FUNCTION_DIMENSION, "Value": function },
        { "Name": RESOURCE_DIMENSION, "Value": function },
    ])
}

/// Every series `ListMetrics` discovers in [`NAMESPACE`], as
/// `(MetricName, [(dimension, value)])` with the dimensions ordered by name — how a
/// client enumerating what it can alarm on reads them.
async fn listed_series(
    router: &MultiServiceRouter,
    case: &str,
) -> Vec<(String, Vec<(String, String)>)> {
    let listed = cloudwatch(
        router,
        case,
        "ListMetrics",
        &json!({ "Namespace": NAMESPACE }),
    )
    .await;
    listed["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|series| {
            let mut dimensions: Vec<(String, String)> = series["Dimensions"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            dimensions.sort();
            (
                series["MetricName"].as_str().unwrap_or_default().to_string(),
                dimensions,
            )
        })
        .collect()
}

/// Which function a row's task names, which decides which series its invocation lands
/// in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The function this suite created, which Lambda resolves and admits an invocation
    /// to.
    Present,
    /// A function no `CreateFunction` made, which Lambda itself refuses.
    Absent,
}

/// One row: the `Resource` the `Task` names, the parameters it carries, which function
/// it addresses, whether the execution role grants the invocation, the invocations the
/// function's series must gain, and the error name the execution must report (`None`
/// where the row asserts nothing about the outcome of the run itself).
struct Row {
    case: &'static str,
    resource: &'static str,
    /// Built per row from the function's ARN or name; `None` for a `Resource` that
    /// names the function itself, whose task carries the execution's input as the
    /// function's event.
    parameters: Option<Value>,
    target: Target,
    granted: bool,
    expected_invocations: f64,
    expected_error: Option<&'static str>,
}

/// RULE: a Lambda-function `Task` really invokes the function, as the execution role,
/// so the function's own `AWS/Lambda` `Invocations` series gains one datapoint per
/// invocation the role was allowed to make and gains nothing for one it was not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lambda_task_invocation_is_counted_in_the_functions_own_series() {
    let router = emulator();
    let function = "handler";
    // The function's own execution role, which Lambda must be able to assume, and then
    // the function itself.
    let function_role = role(
        &router,
        "LambdaExecution",
        LAMBDA_PRINCIPAL,
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "logs:PutLogEvents",
                "Resource": "*",
            }],
        }),
    )
    .await;
    let function_arn = given_function(&router, function, &function_role).await;
    let absent_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:absent-handler");
    // The granting role may invoke ANY function of this account, so a row that is
    // refused by Lambda itself is not confused with one the role was denied.
    let granting = role(
        &router,
        "LambdaTaskGranted",
        STATES_PRINCIPAL,
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": "lambda:InvokeFunction",
                "Resource": format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:*"),
            }],
        }),
    )
    .await;
    let denying = role(
        &router,
        "LambdaTaskDenied",
        STATES_PRINCIPAL,
        &json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["logs:CreateLogDelivery", "logs:PutLogEvents"],
                "Resource": "*",
            }],
        }),
    )
    .await;

    let rows = [
        Row {
            case: "the function's own ARN as the Resource",
            resource: "", // filled in below from the function's ARN
            parameters: None,
            target: Target::Present,
            granted: true,
            expected_invocations: 1.0,
            expected_error: None,
        },
        Row {
            case: "the optimized integration, the function named by its ARN",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: Some(json!({ "FunctionName": function_arn.clone(), "Payload": {} })),
            target: Target::Present,
            granted: true,
            expected_invocations: 1.0,
            expected_error: None,
        },
        Row {
            case: "the AWS SDK integration, the function named by its bare name",
            resource: "arn:aws:states:::aws-sdk:lambda:invoke",
            parameters: Some(json!({ "FunctionName": function, "Payload": {} })),
            target: Target::Present,
            granted: true,
            expected_invocations: 1.0,
            expected_error: None,
        },
        Row {
            case: "the function's own ARN, with a role that may not invoke it",
            resource: "",
            parameters: None,
            target: Target::Present,
            granted: false,
            expected_invocations: 0.0,
            expected_error: Some(LAMBDA_SDK_REFUSAL),
        },
        Row {
            case: "the optimized integration, with a role that may not invoke it",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: Some(json!({ "FunctionName": function_arn.clone(), "Payload": {} })),
            target: Target::Present,
            granted: false,
            expected_invocations: 0.0,
            expected_error: Some(LAMBDA_SDK_REFUSAL),
        },
        Row {
            case: "an ARN naming a function that was never created",
            resource: "", // the absent function's ARN
            parameters: None,
            target: Target::Absent,
            granted: true,
            expected_invocations: 0.0,
            expected_error: Some(LAMBDA_NOT_FOUND),
        },
    ];

    let mut counted = 0.0;
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A `Resource` that names the function itself is that function's ARN, which is
        // minted per row; every other row names an integration token.
        let resource = if row.resource.is_empty() {
            match row.target {
                Target::Present => function_arn.clone(),
                Target::Absent => absent_arn.clone(),
            }
        } else {
            row.resource.to_string()
        };
        let role_arn = if row.granted { &granting } else { &denying };
        let machine_arn = machine(
            &router,
            case,
            &format!("task-{index}"),
            &resource,
            row.parameters.as_ref(),
            role_arn,
        )
        .await;
        let described = ran(&router, case, &machine_arn, &json!({ "order": index })).await;
        if let Some(expected) = row.expected_error {
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("FAILED"),
                "case {case:?}: expected FAILED, got {described}"
            );
            assert_eq!(
                described.get("error").and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected the error {expected}, got {described}"
            );
        }
        // The function's own series carries the invocations the rows so far were
        // allowed to make, and nothing for the ones they were not.
        counted += row.expected_invocations;
        let summed = invocations(&router, case, &function_dimensions(function)).await;
        assert!(
            (summed - counted).abs() < f64::EPSILON,
            "case {case:?}: expected {counted} invocation(s) of {function} in the \
             {NAMESPACE}/{INVOCATIONS_SERIES} series, got {summed}"
        );
        // An unqualified invocation addresses the function itself, so the series
        // reported per invoked resource carries the same count.
        let addressed = invocations(&router, case, &resource_dimensions(function)).await;
        assert!(
            (addressed - counted).abs() < f64::EPSILON,
            "case {case:?}: expected {counted} invocation(s) in the series of the resource the \
             call addressed, got {addressed}"
        );
        // Nothing is published for the function a row only named: a refused
        // invocation leaves no series behind at all.
        let absent = invocations(&router, case, &function_dimensions("absent-handler")).await;
        assert!(
            absent.abs() < f64::EPSILON,
            "case {case:?}: expected no invocation of a function Lambda refused, got {absent}"
        );
    }

    // And the series is DISCOVERABLE: a client enumerating what it can alarm on finds
    // the invoked function's own series, and none for the function nothing invoked.
    let case = "the series a client discovers";
    let listed = listed_series(&router, case).await;
    let expected = (
        INVOCATIONS_SERIES.to_string(),
        vec![(FUNCTION_DIMENSION.to_string(), function.to_string())],
    );
    assert!(
        listed.contains(&expected),
        "case {case:?}: expected {expected:?} among the discovered series, got {listed:?}"
    );
    assert!(
        !listed.iter().any(|(_, dimensions)| dimensions
            .iter()
            .any(|(_, value)| value == "absent-handler")),
        "case {case:?}: expected no series for a function nothing invoked, got {listed:?}"
    );
}
