// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a function's resource-policy surface ADMITS and how it
//! refuses the rest, over sets of inputs rather than one recorded call.
//!
//! The rules under test:
//!
//! * a grant names an identity in one of the documented forms — the `*` wildcard, an
//!   account id, an IAM identity ARN, or a service's domain-style identifier — and a value
//!   in none of them (a service abbreviated to its short name, an ARN of something that is
//!   not an identity) is refused `400 InvalidParameterValueException` with the fixed
//!   sentence, storing nothing;
//! * statements are keyed by `StatementId`: a second grant under a NEW id joins the policy
//!   whatever else it says, while a repeated id is the conflict — and the document reports
//!   them in the order they were granted, each principal in the form it was granted in;
//! * a removal naming a statement id the policy does not hold is `404` naming that id;
//! * a `Qualifier` outside the pattern its member declares is a `400 ValidationException`
//!   stating the value, the member and the expression, decided before any lookup — on every
//!   operation that declares the member — while a qualifier the pattern admits is looked up;
//! * a policy READ that finds no document answers one fixed sentence, whether it is the
//!   policy or the function, version or alias that is absent, and names nothing of the
//!   request.
//!
//! Every row drives real HTTP requests through the router.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction`, `AddPermission` and `GetPolicy` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here. Nothing in it is verified: the harness turns
/// request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The action every grant below names.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The fixed sentence a `Principal` in none of the documented forms is refused with.
const INVALID_PRINCIPAL: &str =
    "The provided principal was invalid. Please check the principal and try again.";

/// The fixed sentence a policy READ answers when the document it was asked for does not
/// exist. Spelled out here rather than read from the crate, so a change to that constant
/// fails this suite.
const NO_DOCUMENT: &str = "The resource you requested does not exist.";

/// The expression a `Qualifier` clause quotes: the member's `@pattern` without the
/// whole-value anchors AWS drops. Source:
/// `smithy_query(query="shape", shape="NumericLatestPublishedOrAliasQualifier")`.
const QUALIFIER_EXPRESSION: &str = r"\$(LATEST(\.PUBLISHED)?)|[a-zA-Z0-9-_$]+";

/// The class every refusal of this surface names in its `Type` member.
const USER_FAULT: &str = "User";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The refusal message, however the envelope spells the member.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_default()
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
            .unwrap_or_default()
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// A fresh router on its own engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
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

/// Create a function.
async fn create_function(router: &MultiServiceRouter, name: &str) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
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

/// `AddPermission` on a function.
async fn add_permission(router: &MultiServiceRouter, function: &str, body: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/policy"),
        body,
    )
    .await
}

/// `GetPolicy` on a function, optionally qualified.
async fn get_policy(
    router: &MultiServiceRouter,
    function: &str,
    qualifier: Option<&str>,
) -> Answer {
    let uri = match qualifier {
        Some(qualifier) => format!(
            "{FUNCTIONS_URI}/{function}/policy?Qualifier={}",
            encoded(qualifier)
        ),
        None => format!("{FUNCTIONS_URI}/{function}/policy"),
    };
    call(router, "GET", &uri, &Value::Null).await
}

/// `RemovePermission` of one statement id.
async fn remove_permission(
    router: &MultiServiceRouter,
    function: &str,
    statement_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("{FUNCTIONS_URI}/{function}/policy/{statement_id}"),
        &Value::Null,
    )
    .await
}

/// `value` percent-encoded for a query parameter, so a qualifier carrying a space or a
/// slash reaches the handler as the caller wrote it.
fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The statements of a reported policy document, in the order the document lists them.
fn statements(case: &str, answer: &Answer) -> Vec<Value> {
    let policy = answer
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Policy member, got {}", answer.text()));
    let document: Value = serde_json::from_str(&policy)
        .unwrap_or_else(|error| panic!("case {case:?}: Policy is not JSON ({error}): {policy}"));
    document
        .get("Statement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One principal row: the value a grant names, and whether it names an identity at all.
struct PrincipalCase {
    case: &'static str,
    principal: &'static str,
    admitted: bool,
    /// The `Principal` element the stored statement carries, for an admitted row.
    expected: Option<Value>,
}

/// The rows, spanning the forms the rule generalises over: each documented form, the
/// domains a service identifier sits under, the identity kinds an ARN names, and the
/// near-misses a client writes.
fn principal_cases() -> Vec<PrincipalCase> {
    vec![
        PrincipalCase {
            case: "every principal",
            principal: "*",
            admitted: true,
            expected: Some(json!("*")),
        },
        PrincipalCase {
            case: "an account id",
            principal: "123456789012",
            admitted: true,
            expected: Some(json!({ "AWS": "arn:aws:iam::123456789012:root" })),
        },
        PrincipalCase {
            case: "an IAM role ARN",
            principal: "arn:aws:iam::123456789012:role/invoker",
            admitted: true,
            expected: Some(json!({ "AWS": "arn:aws:iam::123456789012:role/invoker" })),
        },
        PrincipalCase {
            case: "an assumed-role session ARN",
            principal: "arn:aws:sts::123456789012:assumed-role/invoker/session",
            admitted: true,
            expected: Some(json!({
                "AWS": "arn:aws:sts::123456789012:assumed-role/invoker/session"
            })),
        },
        PrincipalCase {
            case: "a service identifier",
            principal: "apigateway.amazonaws.com",
            admitted: true,
            expected: Some(json!({ "Service": "apigateway.amazonaws.com" })),
        },
        PrincipalCase {
            case: "a service identifier of another partition",
            principal: "s3.amazonaws.com.cn",
            admitted: true,
            expected: Some(json!({ "Service": "s3.amazonaws.com.cn" })),
        },
        PrincipalCase {
            case: "a service identifier outside the amazonaws.com domain",
            principal: "alexa-appkit.amazon.com",
            admitted: true,
            expected: Some(json!({ "Service": "alexa-appkit.amazon.com" })),
        },
        PrincipalCase {
            case: "a regionalised service identifier",
            principal: "logs.us-east-1.amazonaws.com",
            admitted: true,
            expected: Some(json!({ "Service": "logs.us-east-1.amazonaws.com" })),
        },
        // And the values that name nobody.
        PrincipalCase {
            case: "a service abbreviated to its short name",
            principal: "apigateway",
            admitted: false,
            expected: None,
        },
        PrincipalCase {
            case: "another service short name",
            principal: "s3",
            admitted: false,
            expected: None,
        },
        PrincipalCase {
            case: "a service identifier with no domain left",
            principal: "s3.",
            admitted: false,
            expected: None,
        },
        PrincipalCase {
            case: "an account id that is too short",
            principal: "12345678901",
            admitted: false,
            expected: None,
        },
        PrincipalCase {
            case: "an ARN of something that is not an identity",
            principal: "arn:aws:s3:::uploads",
            admitted: false,
            expected: None,
        },
        PrincipalCase {
            case: "an IAM ARN of something that is not an identity",
            principal: "arn:aws:iam::123456789012:policy/invoker",
            admitted: false,
            expected: None,
        },
    ]
}

/// `AddPermission` grants to every documented principal form and refuses every value that
/// names no identity — storing nothing when it refuses.
#[tokio::test]
async fn a_grant_names_an_identity_in_one_of_the_documented_forms() {
    let router = router();

    for (index, row) in principal_cases().into_iter().enumerate() {
        let function = format!("principal-{index}");
        create_function(&router, &function).await;
        let answer = add_permission(
            &router,
            &function,
            &json!({
                "StatementId": "granted",
                "Action": INVOKE_ACTION,
                "Principal": row.principal,
            }),
        )
        .await;

        if row.admitted {
            assert_eq!(
                answer.status,
                201,
                "case {:?}: expected the grant to {:?} to be served, got {} {}",
                row.case,
                row.principal,
                answer.status,
                answer.text()
            );
            // The grant is read back through `GetPolicy`, so a statement that was answered
            // but never stored fails here.
            let read = get_policy(&router, &function, None).await;
            let held = statements(row.case, &read);
            let principal = held
                .first()
                .and_then(|statement| statement.get("Principal"))
                .cloned();
            assert_eq!(
                principal,
                row.expected,
                "case {:?}: expected the stored statement's Principal to be {:?}, got {principal:?}",
                row.case,
                row.expected
            );
            continue;
        }

        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "InvalidParameterValueException".to_string()),
            "case {:?}: expected 400 InvalidParameterValueException for {:?}, got {} {}",
            row.case,
            row.principal,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            INVALID_PRINCIPAL,
            "case {:?}: expected {INVALID_PRINCIPAL:?}, got {:?}",
            row.case,
            answer.message()
        );
        assert_eq!(
            answer.string("Type").as_deref(),
            Some(USER_FAULT),
            "case {:?}: expected the refusal to name the {USER_FAULT} fault class, got {}",
            row.case,
            answer.text()
        );
        // A refused grant stores nothing, so the function still holds no policy at all.
        let read = get_policy(&router, &function, None).await;
        assert_eq!(
            (read.status, read.message()),
            (404, NO_DOCUMENT.to_string()),
            "case {:?}: the refused grant was stored anyway, got {} {}",
            row.case,
            read.status,
            read.text()
        );
    }
}

/// One grant of a table that accumulates: the statement id and what else the grant says.
struct GrantCase {
    case: &'static str,
    statement_id: &'static str,
    extra: Value,
}

/// Statements are keyed by `StatementId` alone: a new id joins the policy however much it
/// repeats of an existing grant, a repeated id is the conflict, and the document reports
/// them in the order they were granted.
#[tokio::test]
async fn statements_are_keyed_by_statement_id_and_reported_in_order() {
    let router = router();
    create_function(&router, "accumulating").await;

    // The rows differ along what the rule generalises over: the same principal and action
    // under a new id, the same grant narrowed to a different source, a different principal
    // form, and a grant that repeats nothing.
    let grants = vec![
        GrantCase {
            case: "the first grant",
            statement_id: "first",
            extra: json!({ "Principal": "apigateway.amazonaws.com" }),
        },
        GrantCase {
            case: "the same principal and action under a new id",
            statement_id: "second",
            extra: json!({ "Principal": "apigateway.amazonaws.com" }),
        },
        GrantCase {
            case: "the same principal narrowed to one source",
            statement_id: "third",
            extra: json!({
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": format!("arn:aws:execute-api:{REGION}:{ACCOUNT}:api/*/GET/items"),
            }),
        },
        GrantCase {
            case: "the same principal narrowed to another source",
            statement_id: "fourth",
            extra: json!({
                "Principal": "apigateway.amazonaws.com",
                "SourceArn": format!("arn:aws:apigateway:{REGION}::/restapis/api/*"),
            }),
        },
        GrantCase {
            case: "a different principal form",
            statement_id: "fifth",
            extra: json!({ "Principal": ACCOUNT }),
        },
    ];

    let mut granted: Vec<&str> = Vec::new();
    for row in &grants {
        let mut body = json!({
            "StatementId": row.statement_id,
            "Action": INVOKE_ACTION,
        });
        if let (Some(target), Some(extra)) = (body.as_object_mut(), row.extra.as_object()) {
            for (member, value) in extra {
                target.insert(member.clone(), value.clone());
            }
        }
        let answer = add_permission(&router, "accumulating", &body).await;
        assert_eq!(
            answer.status,
            201,
            "case {:?}: expected the grant to be served, got {} {}",
            row.case,
            answer.status,
            answer.text()
        );
        assert!(
            answer
                .string("Statement")
                .is_some_and(|statement| statement.contains(row.statement_id)),
            "case {:?}: expected the answered statement to carry the id, got {}",
            row.case,
            answer.text()
        );
        granted.push(row.statement_id);

        // Every grant so far is in the document, in the order they were granted.
        let read = get_policy(&router, "accumulating", None).await;
        let held: Vec<String> = statements(row.case, &read)
            .iter()
            .filter_map(|statement| statement.get("Sid").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert_eq!(
            held, granted,
            "case {:?}: expected the statements {granted:?} in the order granted, got {held:?}",
            row.case
        );
    }

    // A repeated id is the conflict, whatever else the grant says — and it changes nothing.
    for row in &grants {
        let answer = add_permission(
            &router,
            "accumulating",
            &json!({
                "StatementId": row.statement_id,
                "Action": INVOKE_ACTION,
                "Principal": "sns.amazonaws.com",
            }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (409, "ResourceConflictException".to_string()),
            "case {:?}: expected 409 ResourceConflictException for the repeated id, got {} {}",
            row.case,
            answer.status,
            answer.text()
        );
    }
    let read = get_policy(&router, "accumulating", None).await;
    let held: Vec<String> = statements("after the conflicts", &read)
        .iter()
        .filter_map(|statement| statement.get("Sid").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(
        held, granted,
        "case {:?}: the refused grants must change nothing, expected {granted:?}, got {held:?}",
        "after the conflicts"
    );
}

/// A removal naming a statement id the policy does not hold is refused with the sentence
/// that names that id — over several ids, and whatever else the policy holds.
#[tokio::test]
async fn a_removal_of_an_absent_statement_names_the_statement_id() {
    let router = router();
    create_function(&router, "pruned").await;
    let granted = add_permission(
        &router,
        "pruned",
        &json!({
            "StatementId": "held",
            "Action": INVOKE_ACTION,
            "Principal": "sns.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "the grant the policy holds must be served, got {} {}",
        granted.status,
        granted.text()
    );

    // The rows differ along the SHAPE of the id asked for: one never granted, one differing
    // from the held id by case, one that is a prefix of it, and one carrying the `.` the
    // removal's own `NamespacedStatementId` pattern admits.
    for statement_id in ["never-granted", "HELD", "hel", "held.v2", "second"] {
        let answer = remove_permission(&router, "pruned", statement_id).await;
        let expected = format!("Statement {statement_id} is not found in resource policy.");
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {statement_id:?}: expected 404 ResourceNotFoundException, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {statement_id:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
        // The sentence states where it looked rather than naming the resource back.
        assert!(
            !answer.message().contains(&function_arn("pruned")),
            "case {statement_id:?}: the sentence may not name the resource, got {}",
            answer.text()
        );
    }

    // The statement the policy does hold is still removable, so the refusals above took
    // nothing out.
    let removed = remove_permission(&router, "pruned", "held").await;
    assert_eq!(
        removed.status,
        204,
        "case {:?}: expected 204 from the removal of the held statement, got {} {}",
        "the held statement",
        removed.status,
        removed.text()
    );
}

/// One qualifier row: the value, and whether the member's pattern admits it.
struct QualifierCase {
    case: &'static str,
    qualifier: &'static str,
    admitted: bool,
}

/// The rows, spanning what the pattern generalises over: the two forms its first branch
/// names, a published version number, an alias name, and values outside its character
/// class — a space, a slash, a dot that is not `$LATEST.PUBLISHED`, and punctuation.
fn qualifier_cases() -> Vec<QualifierCase> {
    vec![
        QualifierCase {
            case: "the unpublished version",
            qualifier: "$LATEST",
            admitted: true,
        },
        QualifierCase {
            case: "its published reading",
            qualifier: "$LATEST.PUBLISHED",
            admitted: true,
        },
        QualifierCase {
            case: "a published version number",
            qualifier: "1",
            admitted: true,
        },
        QualifierCase {
            case: "an alias name",
            qualifier: "live",
            admitted: true,
        },
        QualifierCase {
            case: "an alias name with the punctuation the class admits",
            qualifier: "live-v1_2",
            admitted: true,
        },
        QualifierCase {
            case: "a value carrying a space",
            qualifier: "bogus q",
            admitted: false,
        },
        QualifierCase {
            case: "a value carrying a slash",
            qualifier: "live/v1",
            admitted: false,
        },
        QualifierCase {
            case: "a value carrying a dot",
            qualifier: "live.v1",
            admitted: false,
        },
        QualifierCase {
            case: "a value carrying a colon",
            qualifier: "live:v1",
            admitted: false,
        },
        QualifierCase {
            case: "a value carrying a plus sign",
            qualifier: "live+v1",
            admitted: false,
        },
    ]
}

/// A `Qualifier` outside the pattern its member declares is the validation refusal, on
/// every operation that declares the member — and a value the pattern admits is looked up
/// instead, so the check is the pattern's rather than a guess at what exists.
#[tokio::test]
async fn a_qualifier_outside_its_pattern_is_refused_before_the_lookup() {
    let router = router();
    create_function(&router, "qualified").await;

    // One row per operation declaring a `NumericLatestPublishedOrAliasQualifier`, each with
    // the method and the URI it is bound to.
    let operations: Vec<(&str, &str, String, Value)> = vec![
        (
            "GetPolicy",
            "GET",
            format!("{FUNCTIONS_URI}/qualified/policy"),
            Value::Null,
        ),
        (
            "AddPermission",
            "POST",
            format!("{FUNCTIONS_URI}/qualified/policy"),
            json!({
                "StatementId": "granted",
                "Action": INVOKE_ACTION,
                "Principal": "sns.amazonaws.com",
            }),
        ),
        (
            "RemovePermission",
            "DELETE",
            format!("{FUNCTIONS_URI}/qualified/policy/granted"),
            Value::Null,
        ),
        (
            "GetFunction",
            "GET",
            format!("{FUNCTIONS_URI}/qualified"),
            Value::Null,
        ),
        (
            "GetFunctionConfiguration",
            "GET",
            format!("{FUNCTIONS_URI}/qualified/configuration"),
            Value::Null,
        ),
        (
            "GetFunctionEventInvokeConfig",
            "GET",
            "/2019-09-25/functions/qualified/event-invoke-config".to_string(),
            Value::Null,
        ),
        (
            "GetRuntimeManagementConfig",
            "GET",
            "/2021-07-20/functions/qualified/runtime-management-config".to_string(),
            Value::Null,
        ),
    ];

    for (operation, method, uri, body) in &operations {
        for row in qualifier_cases() {
            let separator = if uri.contains('?') { '&' } else { '?' };
            let addressed = format!(
                "{uri}{separator}Qualifier={}",
                encoded(row.qualifier)
            );
            let answer = call(&router, method, &addressed, body).await;
            let case = format!("{operation} with {}: {}", row.case, row.qualifier);

            if row.admitted {
                assert_ne!(
                    (answer.status, answer.error_code()),
                    (400, "ValidationException".to_string()),
                    "case {case:?}: the pattern admits this value, so it must be looked up \
                     rather than refused, got {} {}",
                    answer.status,
                    answer.text()
                );
                continue;
            }

            assert_eq!(
                (answer.status, answer.error_code()),
                (400, "ValidationException".to_string()),
                "case {case:?}: expected 400 ValidationException, got {} {}",
                answer.status,
                answer.text()
            );
            let expected = format!(
                "1 validation error detected: Value '{}' at 'qualifier' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: \
                 {QUALIFIER_EXPRESSION}",
                row.qualifier
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }

    // The refusals ran before any lookup, so none of them left a grant behind and the
    // function still holds no policy.
    let read = get_policy(&router, "qualified", None).await;
    assert_eq!(
        (read.status, read.message()),
        (404, NO_DOCUMENT.to_string()),
        "case {:?}: a refused request must store nothing, got {} {}",
        "after the refusals",
        read.status,
        read.text()
    );
}

/// A policy READ that finds no document states one fixed sentence, whether it is the policy
/// or the resource that is absent — and a `FunctionName` whose own qualifier disagrees with
/// the `Qualifier` parameter is the cross-check's own fixed sentence instead.
#[tokio::test]
async fn a_policy_read_with_no_document_states_one_sentence() {
    let router = router();
    create_function(&router, "readable").await;

    // The rows differ along WHAT is absent: the policy of a resource that resolves, the
    // function itself, a version of a function that exists, and an alias of one.
    let rows: Vec<(&str, String)> = vec![
        (
            "a function that holds no policy",
            format!("{FUNCTIONS_URI}/readable/policy"),
        ),
        (
            "a function that does not exist",
            format!("{FUNCTIONS_URI}/never-created/policy"),
        ),
        (
            "a version that does not exist",
            format!("{FUNCTIONS_URI}/readable/policy?Qualifier=9"),
        ),
        (
            "an alias that does not exist",
            format!("{FUNCTIONS_URI}/readable/policy?Qualifier=ghost"),
        ),
        (
            "a version named in the function name",
            format!("{FUNCTIONS_URI}/readable:9/policy"),
        ),
        (
            "a function of this account addressed by its partial ARN",
            format!("{FUNCTIONS_URI}/{ACCOUNT}:function:never-created/policy"),
        ),
    ];

    for (case, uri) in &rows {
        let answer = call(&router, "GET", uri, &Value::Null).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, "ResourceNotFoundException".to_string()),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            NO_DOCUMENT,
            "case {case:?}: expected {NO_DOCUMENT:?}, got {:?}",
            answer.message()
        );
    }

    // The cross-check between the two halves of the request is a different condition, and
    // a 400: both halves came from the caller, so the sentence states only that they
    // disagree.
    let mismatch = format!("{FUNCTIONS_URI}/readable:1/policy?Qualifier=2");
    let answer = call(&router, "GET", &mismatch, &Value::Null).await;
    assert_eq!(
        (answer.status, answer.message()),
        (
            400,
            "The derived qualifier from the function name does not match the specified \
             qualifier."
                .to_string()
        ),
        "case {:?}: expected the qualifier cross-check's fixed sentence, got {} {}",
        "a function name and a qualifier that disagree",
        answer.status,
        answer.text()
    );
}
