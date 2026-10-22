// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a function's resource-based policy: `AddPermission` grants a
//! principal permission to use a function, and `GetPolicy` reports the policy those
//! grants make up.
//!
//! The rule under test: `AddPermission` appends to the resource-based policy of the ARN
//! the request names exactly one statement describing the grant the request asked for —
//! its `Sid` is the `StatementId`, its `Effect` is `Allow`, its `Principal` is the
//! supplied principal in the form the policy language spells it, its `Action` is the
//! supplied action, its `Resource` is the ARN the permission was granted on, and every
//! optional member of the request becomes the IAM condition key that expresses it —
//! answers that statement, and `GetPolicy` reports the document of the statements that
//! resource has accumulated, at the revision the last change minted.
//!
//! Every row drives real HTTP requests through the router and reads the grant back
//! through `GetPolicy`, so a statement that was answered but never stored fails here.
//! The rows differ along what the rule generalises over: the form of the principal
//! (service, account, role ARN, every principal), the action, which restricting members
//! are present, and whether the grant is function-level or on a published version.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off, exactly as
/// the emulator runs unless started with it on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The IAM policy language version a policy document carries, and the `Id` Lambda gives
/// the policy it maintains for a function.
const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";
const POLICY_ID: &str = "default";

/// The published quota on a function's resource-based policy, in bytes (Lambda quotas,
/// "Function resource-based policy": 20 KB).
const MAX_POLICY_SIZE_BYTES: usize = 20_480;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body. Panics with the body text
    /// when it is neither, which is the failure a caller needs to see.
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

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
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
        headers: parts.headers,
        body,
    }
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The `?Qualifier=` suffix for a qualified call, and nothing for a function-level one.
fn qualifier_query(qualifier: Option<&str>) -> String {
    qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"))
}

/// `AddPermission` on a function, optionally qualified.
async fn add_permission(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
    body: &Value,
) -> Answer {
    let uri = format!(
        "{FUNCTIONS_URI}/{function}/policy{}",
        qualifier_query(qualifier)
    );
    call(router, "POST", &uri, body).await
}

/// `GetPolicy` on a function, optionally qualified.
async fn get_policy(router: &MultiServiceRouter, function: &str, qualifier: Option<&str>) -> Answer {
    let uri = format!(
        "{FUNCTIONS_URI}/{function}/policy{}",
        qualifier_query(qualifier)
    );
    call(router, "GET", &uri, &Value::Null).await
}

/// Create a function, publishing version 1 when asked.
async fn create_function(router: &MultiServiceRouter, name: &str, publish: bool) {
    let package: Vec<u8> = (0..64u16).map(|byte| u8::try_from(byte % 251).unwrap_or(0)).collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": publish,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// The `Statement` member of an `AddPermission` answer, parsed.
fn statement_of(case: &str, answer: &Answer) -> Value {
    let statement = answer
        .string("Statement")
        .unwrap_or_else(|| panic!("case {case:?}: no Statement member in {}", answer.text()));
    serde_json::from_str(&statement).unwrap_or_else(|error| {
        panic!("case {case:?}: Statement is not JSON ({error}): {statement:?}")
    })
}

/// The `Policy` member of a `GetPolicy` answer, parsed.
fn policy_of(case: &str, answer: &Answer) -> Value {
    let policy = answer
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case:?}: no Policy member in {}", answer.text()));
    serde_json::from_str(&policy)
        .unwrap_or_else(|error| panic!("case {case:?}: Policy is not JSON ({error}): {policy:?}"))
}

/// One row of the grant table: the function to grant on, the request, and the statement
/// the policy must then hold.
struct GrantCase {
    name: &'static str,
    function: &'static str,
    publish: bool,
    qualifier: Option<&'static str>,
    request: Value,
    expected_statement: Value,
}

/// The rows, each differing from the others in the form of its principal, its action or
/// the restricting members it carries — the dimensions the rule generalises over. The
/// `Resource` of each is the ARN the permission is granted on, so the qualified row's
/// ends in its version.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        // A service principal limited to one invoking resource: the shape every
        // cross-service integration (CodeCommit triggers, S3 notifications, SNS
        // subscriptions) uses.
        GrantCase {
            name: "a service principal limited to one source resource",
            function: "trigger-target",
            publish: false,
            qualifier: None,
            request: json!({
                "StatementId": "codecommit-invoke",
                "Action": "lambda:InvokeFunction",
                "Principal": "codecommit.amazonaws.com",
                "SourceArn": format!("arn:aws:codecommit:{REGION}:{ACCOUNT}:demo-repo"),
            }),
            expected_statement: json!({
                "Sid": "codecommit-invoke",
                "Effect": "Allow",
                "Principal": { "Service": "codecommit.amazonaws.com" },
                "Action": "lambda:InvokeFunction",
                "Resource": function_arn("trigger-target"),
                "Condition": {
                    "ArnLike": {
                        "AWS:SourceArn": format!("arn:aws:codecommit:{REGION}:{ACCOUNT}:demo-repo"),
                    },
                },
            }),
        },
        // An account id is that account's root principal, written as its ARN; nothing
        // restricts the grant, so the statement carries no condition at all.
        GrantCase {
            name: "an account principal with every lambda action",
            function: "account-grant",
            publish: false,
            qualifier: None,
            request: json!({
                "StatementId": "whole-account",
                "Action": "lambda:*",
                "Principal": "111122223333",
            }),
            expected_statement: json!({
                "Sid": "whole-account",
                "Effect": "Allow",
                "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
                "Action": "lambda:*",
                "Resource": function_arn("account-grant"),
            }),
        },
        // An IAM role is already an ARN and is used as given, and `SourceAccount` is
        // compared for equality.
        GrantCase {
            name: "a role principal limited to the account that owns the source",
            function: "role-grant",
            publish: false,
            qualifier: None,
            request: json!({
                "StatementId": "role-reads-config",
                "Action": "lambda:GetFunction",
                "Principal": "arn:aws:iam::444455556666:role/invoker",
                "SourceAccount": "444455556666",
            }),
            expected_statement: json!({
                "Sid": "role-reads-config",
                "Effect": "Allow",
                "Principal": { "AWS": "arn:aws:iam::444455556666:role/invoker" },
                "Action": "lambda:GetFunction",
                "Resource": function_arn("role-grant"),
                "Condition": {
                    "StringEquals": { "AWS:SourceAccount": "444455556666" },
                },
            }),
        },
        // Every principal, narrowed to one organization.
        GrantCase {
            name: "every principal in one organization",
            function: "org-grant",
            publish: false,
            qualifier: None,
            request: json!({
                "StatementId": "org-wide",
                "Action": "*",
                "Principal": "*",
                "PrincipalOrgID": "o-abcdefghij",
            }),
            expected_statement: json!({
                "Sid": "org-wide",
                "Effect": "Allow",
                "Principal": "*",
                "Action": "*",
                "Resource": function_arn("org-grant"),
                "Condition": {
                    "StringEquals": { "aws:PrincipalOrgID": "o-abcdefghij" },
                },
            }),
        },
        // Three restrictions at once, including the boolean one: every optional member
        // becomes its own condition key, grouped by the operator that compares it.
        GrantCase {
            name: "a function-URL grant carrying an event source token",
            function: "url-grant",
            publish: false,
            qualifier: None,
            request: json!({
                "StatementId": "alexa-skill",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "alexa-appkit.amazon.com",
                "EventSourceToken": "amzn1.ask.skill.abc-123",
                "FunctionUrlAuthType": "NONE",
                "InvokedViaFunctionUrl": true,
            }),
            expected_statement: json!({
                "Sid": "alexa-skill",
                "Effect": "Allow",
                "Principal": { "Service": "alexa-appkit.amazon.com" },
                "Action": "lambda:InvokeFunctionUrl",
                "Resource": function_arn("url-grant"),
                "Condition": {
                    "StringEquals": {
                        "lambda:EventSourceToken": "amzn1.ask.skill.abc-123",
                        "lambda:FunctionUrlAuthType": "NONE",
                    },
                    "Bool": { "lambda:InvokedViaFunctionUrl": "true" },
                },
            }),
        },
        // "You can apply the policy at the function level, or specify a qualifier to
        // restrict access to a single version": the policy is attached to the qualified
        // ARN, which is the statement's resource.
        GrantCase {
            name: "a published version named by the Qualifier parameter",
            function: "versioned-grant",
            publish: true,
            qualifier: Some("1"),
            request: json!({
                "StatementId": "sns-invokes-v1",
                "Action": "lambda:InvokeFunction",
                "Principal": "sns.amazonaws.com",
                "SourceArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:releases"),
            }),
            expected_statement: json!({
                "Sid": "sns-invokes-v1",
                "Effect": "Allow",
                "Principal": { "Service": "sns.amazonaws.com" },
                "Action": "lambda:InvokeFunction",
                "Resource": format!("{}:1", function_arn("versioned-grant")),
                "Condition": {
                    "ArnLike": { "AWS:SourceArn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:releases") },
                },
            }),
        },
    ]
}

/// `AddPermission` answers the statement it adds, and `GetPolicy` reports the policy it
/// was added to.
#[tokio::test]
async fn add_permission_writes_the_grant_the_request_describes() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in grant_cases() {
        create_function(&router, case.function, case.publish).await;

        let answer = add_permission(&router, case.function, case.qualifier, &case.request).await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: expected HTTP 201 from AddPermission, got {} {}",
            case.name,
            answer.status,
            answer.text()
        );
        let statement = statement_of(case.name, &answer);
        assert_eq!(
            statement, case.expected_statement,
            "case {:?}: expected the statement {}, got {statement}",
            case.name, case.expected_statement
        );

        // The grant is readable back: the policy of that resource holds exactly the one
        // statement, at a revision the call minted.
        let read = get_policy(&router, case.function, case.qualifier).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected HTTP 200 from GetPolicy, got {} {}",
            case.name,
            read.status,
            read.text()
        );
        let policy = policy_of(case.name, &read);
        assert_eq!(
            policy,
            json!({
                "Version": POLICY_LANGUAGE_VERSION,
                "Id": POLICY_ID,
                "Statement": [case.expected_statement],
            }),
            "case {:?}: expected the policy document to hold just that statement, got {policy}",
            case.name
        );
        let revision = read.string("RevisionId").unwrap_or_default();
        assert!(
            !revision.is_empty(),
            "case {:?}: expected GetPolicy to report a RevisionId, got {}",
            case.name,
            read.text()
        );

        // A report must not change what a later caller sees.
        let again = get_policy(&router, case.function, case.qualifier).await;
        assert_eq!(
            (again.status, policy_of(case.name, &again), again.string("RevisionId")),
            (read.status, policy.clone(), read.string("RevisionId")),
            "case {:?}: reading the policy changed it: {} {}",
            case.name,
            again.status,
            again.text()
        );
    }
}

/// Statements accumulate on the resource they were granted on, each change mints a new
/// revision, and two qualifiers of one function never share a policy.
#[tokio::test]
async fn statements_accumulate_per_resource_and_each_change_mints_a_revision() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "fan-in", true).await;

    // The principals of three services that commonly invoke one function: the row
    // differs only in the grant, which is what accumulates.
    let grants = [
        ("s3-invoke", "s3.amazonaws.com"),
        ("sns-invoke", "sns.amazonaws.com"),
        ("events-invoke", "events.amazonaws.com"),
    ];

    let mut expected: Vec<Value> = Vec::new();
    let mut revisions: Vec<String> = Vec::new();
    for (sid, principal) in grants {
        let answer = add_permission(
            &router,
            "fan-in",
            None,
            &json!({
                "StatementId": sid,
                "Action": "lambda:InvokeFunction",
                "Principal": principal,
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {sid:?}: expected HTTP 201, got {} {}",
            answer.status,
            answer.text()
        );
        expected.push(json!({
            "Sid": sid,
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "lambda:InvokeFunction",
            "Resource": function_arn("fan-in"),
        }));

        let read = get_policy(&router, "fan-in", None).await;
        let policy = policy_of(sid, &read);
        assert_eq!(
            policy,
            json!({
                "Version": POLICY_LANGUAGE_VERSION,
                "Id": POLICY_ID,
                "Statement": expected.clone(),
            }),
            "case {sid:?}: expected the policy to hold the {} statements added so far, got {policy}",
            expected.len()
        );
        let revision = read.string("RevisionId").unwrap_or_default();
        assert!(
            !revisions.contains(&revision),
            "case {sid:?}: expected a new RevisionId after a change, got the earlier {revision:?}"
        );
        revisions.push(revision);
    }

    // Version 1's policy is its own: granting on it leaves the function-level policy
    // alone, and its own document holds only its statement.
    let qualified = add_permission(
        &router,
        "fan-in",
        Some("1"),
        &json!({
            "StatementId": "sqs-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "sqs.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        qualified.status,
        201,
        "case {:?}: expected HTTP 201, got {} {}",
        "a grant on version 1",
        qualified.status,
        qualified.text()
    );
    let version_policy = policy_of("a grant on version 1", &get_policy(&router, "fan-in", Some("1")).await);
    assert_eq!(
        version_policy,
        json!({
            "Version": POLICY_LANGUAGE_VERSION,
            "Id": POLICY_ID,
            "Statement": [json!({
                "Sid": "sqs-invoke",
                "Effect": "Allow",
                "Principal": { "Service": "sqs.amazonaws.com" },
                "Action": "lambda:InvokeFunction",
                "Resource": format!("{}:1", function_arn("fan-in")),
            })],
        }),
        "case {:?}: expected version 1's own policy, got {version_policy}",
        "a grant on version 1"
    );
    let function_policy = policy_of("the function-level policy", &get_policy(&router, "fan-in", None).await);
    assert_eq!(
        function_policy.pointer("/Statement").and_then(Value::as_array).map(Vec::len),
        Some(3),
        "case {:?}: expected the function-level policy to still hold its 3 statements, \
         got {function_policy}",
        "the function-level policy"
    );

    // The statements live on the stored record, not in some per-call scratch state.
    let held = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .functions
        .get("fan-in")
        .map(|record| {
            record
                .policies
                .iter()
                .map(|(qualifier, policy)| (qualifier.clone(), policy.statements.len()))
                .collect::<Vec<_>>()
        });
    assert_eq!(
        held,
        Some(vec![("$LATEST".to_string(), 3), ("1".to_string(), 1)]),
        "expected the store to hold three function-level statements and one on version 1, \
         got {held:?}"
    );
}

/// One row of the refusal table: the call to make, and the status and wire error code it
/// must answer.
struct RefusalCase {
    name: &'static str,
    function: &'static str,
    qualifier: Option<&'static str>,
    body: Value,
    status: u16,
    code: &'static str,
}

/// Every refusal the slice makes, each from the state the store holds.
#[tokio::test]
async fn add_permission_and_get_policy_refuse_what_they_cannot_grant_or_report() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "guarded", false).await;
    create_function(&router, "policy-less", false).await;

    let grant = json!({
        "StatementId": "first",
        "Action": "lambda:InvokeFunction",
        "Principal": "s3.amazonaws.com",
    });
    let first = add_permission(&router, "guarded", None, &grant).await;
    assert_eq!(
        first.status,
        201,
        "the first grant must succeed, got {} {}",
        first.status,
        first.text()
    );

    // Each row: what is asked for, and the status and error code it must answer.
    let refusals = vec![
        RefusalCase {
            name: "an unknown function",
            function: "no-such-function",
            qualifier: None,
            body: grant.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "an unknown version of a known function",
            function: "guarded",
            qualifier: Some("7"),
            body: grant.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "a statement id the policy already holds",
            function: "guarded",
            qualifier: None,
            body: grant.clone(),
            status: 409,
            code: "ResourceConflictException",
        },
        RefusalCase {
            name: "the unpublished version named as the qualifier",
            function: "guarded",
            qualifier: Some("$LATEST"),
            body: json!({
                "StatementId": "on-latest",
                "Action": "lambda:InvokeFunction",
                "Principal": "s3.amazonaws.com",
            }),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a RevisionId that is not the policy's",
            function: "guarded",
            qualifier: None,
            body: json!({
                "StatementId": "stale",
                "Action": "lambda:InvokeFunction",
                "Principal": "sns.amazonaws.com",
                "RevisionId": "00000000-0000-4000-8000-000000000000",
            }),
            status: 412,
            code: "PreconditionFailedException",
        },
        RefusalCase {
            name: "a RevisionId on a function that has no policy yet",
            function: "policy-less",
            qualifier: None,
            body: json!({
                "StatementId": "stale",
                "Action": "lambda:InvokeFunction",
                "Principal": "sns.amazonaws.com",
                "RevisionId": "00000000-0000-4000-8000-000000000000",
            }),
            status: 412,
            code: "PreconditionFailedException",
        },
    ];

    for case in refusals {
        let answer = add_permission(&router, case.function, case.qualifier, &case.body).await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.code),
            "case {:?}: expected {}, got {}",
            case.name,
            case.code,
            answer.error_code()
        );
    }

    // A refused call changes nothing: the policy still holds only the statement that
    // was granted, at the revision that granted it.
    let read = get_policy(&router, "guarded", None).await;
    let policy = policy_of("after the refusals", &read);
    assert_eq!(
        policy
            .pointer("/Statement")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "case {:?}: expected the one granted statement, got {policy}",
        "after the refusals"
    );
    let revision = read
        .string("RevisionId")
        .unwrap_or_else(|| panic!("expected a RevisionId, got {}", read.text()));

    // "Update the policy only if the revision ID matches the ID that's specified": the
    // revision GetPolicy reports is the one that matches.
    let matched = add_permission(
        &router,
        "guarded",
        None,
        &json!({
            "StatementId": "second",
            "Action": "lambda:InvokeFunction",
            "Principal": "sns.amazonaws.com",
            "RevisionId": revision,
        }),
    )
    .await;
    assert_eq!(
        matched.status,
        201,
        "case {:?}: expected HTTP 201, got {} {}",
        "a RevisionId that is the policy's",
        matched.status,
        matched.text()
    );

    // A function, version or alias with no policy has nothing to report.
    for (case, function, qualifier) in [
        ("a function that has no policy", "policy-less", None),
        ("an unknown function", "no-such-function", None),
        ("an unknown version", "guarded", Some("7")),
    ] {
        let answer = get_policy(&router, function, qualifier).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected HTTP 404 from GetPolicy, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }
}

/// A grant that would push the document past the published 20 KB quota is refused, and
/// the policy it would have grown is left exactly as it was.
#[tokio::test]
async fn a_policy_cannot_grow_past_its_published_quota() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "bulky", false).await;

    // One statement of about 11 KB: the action and the source ARN are each within their
    // modelled length and pattern, so the boundary validator accepts them and the
    // quota is what refuses the second one.
    let action = format!("lambda:{}", "A".repeat(9_000));
    let source_arn = format!("arn:aws:s3:::{}", "b".repeat(1_500));
    let body = |sid: &str| {
        json!({
            "StatementId": sid,
            "Action": action,
            "Principal": "s3.amazonaws.com",
            "SourceArn": source_arn,
        })
    };

    let first = add_permission(&router, "bulky", None, &body("first")).await;
    assert_eq!(
        first.status,
        201,
        "case {:?}: expected HTTP 201 while the policy fits, got {} {}",
        "one large statement",
        first.status,
        first.text()
    );
    let one_statement = policy_of("one large statement", &get_policy(&router, "bulky", None).await);
    let size = one_statement.to_string().len();
    assert!(
        size <= MAX_POLICY_SIZE_BYTES && size * 2 > MAX_POLICY_SIZE_BYTES,
        "case {:?}: this test needs one statement to fit and two not to, but the document \
         is {size} bytes against a {MAX_POLICY_SIZE_BYTES} byte quota",
        "one large statement"
    );

    let second = add_permission(&router, "bulky", None, &body("second")).await;
    assert_eq!(
        second.status,
        400,
        "case {:?}: expected HTTP 400 past the quota, got {} {}",
        "a second large statement",
        second.status,
        second.text()
    );
    assert!(
        second.error_code().contains("PolicyLengthExceededException"),
        "case {:?}: expected PolicyLengthExceededException, got {}",
        "a second large statement",
        second.error_code()
    );

    let after = policy_of("after the refusal", &get_policy(&router, "bulky", None).await);
    assert_eq!(
        after, one_statement,
        "case {:?}: the refused grant changed the policy, which is now {after}",
        "after the refusal"
    );
}
