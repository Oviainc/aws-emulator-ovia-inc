// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ARN a `FunctionConfiguration` reports, and in particular for
//! the one a call that PUBLISHES a version as a side effect of its own `Publish` parameter
//! reports.
//!
//! The rule under test: `FunctionArn` names the resource the call's subject IS. A call
//! addressed to the function itself — `CreateFunction`, `UpdateFunctionCode` — reports the
//! function's own unqualified ARN (`arn:<partition>:lambda:<region>:<account>:function:<name>`)
//! whether or not it also published a version, and names that version by `Version` alone;
//! a call whose subject is a QUALIFIER — `PublishVersion`, which creates the version, or a
//! read addressed by one — reports that qualifier's ARN
//! (`<function-arn>:<version>`). So `FunctionArn` plus `":" + Version` is the version ARN,
//! qualified once rather than twice, which is how a client turns a create into the ARN of
//! the version it just published.
//!
//! Every row drives real HTTP requests through the router and reads the function back
//! through `GetFunction`, unqualified and qualified, so a reported ARN that no read agrees
//! with fails here. The rows differ along what the rule generalises over: the operation
//! (create, code update), whether it publishes, how many versions the function already
//! held (so the published number varies), and the documented `FunctionName` form the
//! request addresses the function by.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and what region the minted ARNs carry. Nothing in
/// it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version's label, from the model's `Version` pattern
/// (`smithy_query(query="shape", shape="Version")`: `^(\$LATEST|[0-9]+)$`).
const VERSION_LATEST: &str = "$LATEST";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The value at `pointer`, or `Value::Null` when the body carries none there.
    fn at(&self, pointer: &str) -> Value {
        self.json().pointer(pointer).cloned().unwrap_or(Value::Null)
    }

    /// The string at `pointer`, when the body carries one there.
    fn string(&self, pointer: &str) -> Option<String> {
        self.at(pointer).as_str().map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// Deployment package bytes of a given length. Only the length and the digest are
/// reported, but the content differs per length so two packages never share a digest.
fn package_bytes(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The unqualified ARN of the function `name`.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `CreateFunction` naming the function by `reference` (one of the documented
/// `FunctionName` forms), publishing version 1 when `publish` says so.
async fn create_function(
    router: &MultiServiceRouter,
    reference: &str,
    publish: Option<bool>,
) -> Answer {
    let mut body = json!({
        "FunctionName": reference,
        "Role": ROLE,
        "Runtime": "python3.13",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(publish) = publish {
        body["Publish"] = json!(publish);
    }
    call(router, "POST", FUNCTIONS_URI, &body).await
}

/// `UpdateFunctionCode` with a package of its own, publishing when `publish` says so.
async fn update_function_code(
    router: &MultiServiceRouter,
    reference: &str,
    length: usize,
    publish: bool,
) -> Answer {
    call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{reference}/code"),
        &json!({ "ZipFile": zip_file(&package_bytes(length)), "Publish": publish }),
    )
    .await
}

/// `PublishVersion` with a description, so each call is a change the function has not
/// published yet ("Lambda doesn't publish a version if the function's configuration and
/// code haven't changed since the last version").
async fn publish_version(router: &MultiServiceRouter, reference: &str, round: u32) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/versions"),
        &json!({ "Description": format!("snapshot {round}") }),
    )
    .await
}

/// `GetFunction`, unqualified or qualified by `version`.
async fn get_function(
    router: &MultiServiceRouter,
    reference: &str,
    version: Option<&str>,
) -> Answer {
    let uri = match version {
        Some(version) => format!("{FUNCTIONS_URI}/{reference}?Qualifier={version}"),
        None => format!("{FUNCTIONS_URI}/{reference}"),
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting — `UpdateFunctionCode` is refused while the function is
/// still being provisioned.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// The operation whose answer a row inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Subject {
    /// `CreateFunction`, with the `Publish` member as stated (`None`: omitted).
    Create(Option<bool>),
    /// `UpdateFunctionCode` of a function that already holds `versions` versions, with
    /// the `Publish` member as stated.
    UpdateCode { versions: u32, publish: bool },
}

/// One row: the call under test, the `FunctionName` form it is addressed by, and the
/// `Version` its answer must report.
struct Case {
    name: &'static str,
    subject: Subject,
    /// The `FunctionName` form, built from the function name; `None` means the bare name.
    reference: Option<fn(&str) -> String>,
    /// The `Version` the answer must name, which for a publishing call is the version it
    /// published.
    version: &'static str,
}

/// The rows. Between them they cover both operations that take a `Publish`, a publish and
/// no publish (and, for the create, the member omitted entirely), a function that held no
/// version and one that held one before the call (so the published number is 1 and 2),
/// and each documented `FunctionName` form.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "create publishing version 1, bare name",
            subject: Subject::Create(Some(true)),
            reference: None,
            version: "1",
        },
        Case {
            name: "create publishing version 1, full ARN",
            subject: Subject::Create(Some(true)),
            reference: Some(|name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")),
            version: "1",
        },
        Case {
            name: "create without publishing, bare name",
            subject: Subject::Create(Some(false)),
            reference: None,
            version: VERSION_LATEST,
        },
        Case {
            name: "create with Publish omitted, name with the resource-type prefix",
            subject: Subject::Create(None),
            reference: Some(|name| format!("function:{name}")),
            version: VERSION_LATEST,
        },
        Case {
            name: "code update publishing the first version, bare name",
            subject: Subject::UpdateCode {
                versions: 0,
                publish: true,
            },
            reference: None,
            version: "1",
        },
        Case {
            name: "code update publishing version 3, partial ARN",
            subject: Subject::UpdateCode {
                versions: 2,
                publish: true,
            },
            reference: Some(|name| format!("{ACCOUNT}:function:{name}")),
            version: "3",
        },
        Case {
            name: "code update without publishing, bare name",
            subject: Subject::UpdateCode {
                versions: 1,
                publish: false,
            },
            reference: None,
            version: VERSION_LATEST,
        },
    ]
}

/// Every row: the call reports the function's own unqualified ARN, names the version it
/// published in `Version` alone, and the version ARN a client forms from the two is the
/// one a qualified read of that version reports.
#[tokio::test]
async fn a_call_addressed_to_the_function_reports_the_unqualified_arn() {
    for case in cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let function = "subject-function";
        let reference = case
            .reference
            .map_or_else(|| function.to_string(), |build| build(function));

        let (answer, expected_status) = match case.subject {
            // The create names the function by the row's own `FunctionName` form; the
            // code update names it in the request URI instead, so that one is created by
            // its bare name.
            Subject::Create(publish) => (create_function(&router, &reference, publish).await, 201),
            Subject::UpdateCode { versions, publish } => {
                create_function(&router, function, Some(versions >= 1)).await;
                for round in 2..=versions {
                    publish_version(&router, function, round).await;
                }
                make_activation_due(&engine);
                (
                    // A package of its own length, so the code really changed and the
                    // publish is of a configuration the function has not published yet.
                    update_function_code(&router, &reference, 128, publish).await,
                    200,
                )
            }
        };

        assert_eq!(
            answer.status,
            expected_status,
            "case {:?}: expected {expected_status}, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.string("/FunctionArn"),
            Some(function_arn(function)),
            "case {:?}: expected the function's unqualified ARN {:?}, got {:?}",
            case.name,
            function_arn(function),
            answer.string("/FunctionArn")
        );
        assert_eq!(
            answer.string("/Version").as_deref(),
            Some(case.version),
            "case {:?}: expected Version {:?}, got {:?}",
            case.name,
            case.version,
            answer.string("/Version")
        );

        // The unqualified read agrees with what the call reported.
        let unqualified = get_function(&router, function, None).await;
        assert_eq!(
            unqualified.string("/Configuration/FunctionArn"),
            Some(function_arn(function)),
            "case {:?}: an unqualified read must report the unqualified ARN, got {}",
            case.name,
            unqualified.text()
        );

        if case.version == VERSION_LATEST {
            continue;
        }

        // The version ARN a client forms out of the answer — its `FunctionArn` qualified
        // by its `Version` — is the ARN the qualified read of that version reports, so the
        // two never disagree by a qualifier.
        let composed = format!("{}:{}", function_arn(function), case.version);
        let qualified = get_function(&router, function, Some(case.version)).await;
        assert_eq!(
            qualified.string("/Configuration/FunctionArn"),
            Some(composed.clone()),
            "case {:?}: expected the qualified read to report {composed:?}, got {}",
            case.name,
            qualified.text()
        );
        assert_eq!(
            qualified.string("/Configuration/Version").as_deref(),
            Some(case.version),
            "case {:?}: expected the qualified read to report Version {:?}, got {}",
            case.name,
            case.version,
            qualified.text()
        );
    }
}

/// The other half of the rule: a call whose subject is a QUALIFIER reports that
/// qualifier's own ARN. `PublishVersion` creates the version, so it names it; so does
/// every read addressed by one.
#[tokio::test]
async fn a_call_addressed_to_a_qualifier_reports_the_qualified_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "qualifier-subject";
    create_function(&router, function, Some(false)).await;
    make_activation_due(&engine);

    // Three versions, so the assertion holds for more than the first number.
    for round in 1..=3u32 {
        let answer = publish_version(&router, function, round).await;
        assert_eq!(
            answer.status,
            201,
            "round {round}: PublishVersion must answer 201, got {} {}",
            answer.status,
            answer.text()
        );
        let expected = format!("{}:{round}", function_arn(function));
        assert_eq!(
            answer.string("/FunctionArn"),
            Some(expected.clone()),
            "round {round}: expected PublishVersion to report {expected:?}, got {}",
            answer.text()
        );
        assert_eq!(
            answer.string("/Version").as_deref(),
            Some(round.to_string().as_str()),
            "round {round}: expected Version {round:?}, got {}",
            answer.text()
        );

        let read = get_function(&router, function, Some(&round.to_string())).await;
        assert_eq!(
            read.string("/Configuration/FunctionArn"),
            Some(expected.clone()),
            "round {round}: expected the qualified read to report {expected:?}, got {}",
            read.text()
        );
    }

    // And the unpublished configuration, which every one of those versions was taken
    // from, still reports the function's own ARN.
    let latest = get_function(&router, function, Some(VERSION_LATEST)).await;
    assert_eq!(
        latest.string("/Configuration/FunctionArn"),
        Some(function_arn(function)),
        "the unpublished version must report the unqualified ARN, got {}",
        latest.text()
    );
}
