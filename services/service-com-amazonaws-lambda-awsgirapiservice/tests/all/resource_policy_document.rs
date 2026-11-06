// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the resource-policy document surface: `PutResourcePolicy`,
//! `GetResourcePolicy` and `DeleteResourcePolicy`.
//!
//! The rule under test: each of these operations addresses ONE Lambda resource by the
//! ARN in its path — a function, or a version or alias of one — and acts on that
//! resource's single resource-based policy. `PutResourcePolicy` stores the WHOLE document
//! the request carried, at a revision it mints, replacing whatever policy the resource
//! had ("This operation replaces any existing policy on the Lambda resource. If you
//! previously added permissions using the AddPermission operation, the new policy
//! overwrites those permissions"); `GetResourcePolicy` reports exactly that document and
//! that revision; `DeleteResourcePolicy` removes it; and a `RevisionId` given to either
//! writer must equal the stored policy's current one for the call to proceed.
//!
//! Every row drives real HTTP requests through the router and reads the policy back, so a
//! document that was answered but never stored fails here. The rows differ along what the
//! rule generalises over: the form and size of the document (one statement, several, a
//! single statement object rather than a list, members Lambda itself never writes, a
//! document near the published quota), the resource the ARN names (the function, its
//! unpublished version named explicitly, a published version, an alias) and whether a
//! revision precondition is carried.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where the resource-policy operations are bound
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`).
const RESOURCE_POLICY_URI: &str = "/2026-07-09/resource-policy";

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

/// The unpublished version label, as the model's `Version` pattern spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The published quota on a resource-based policy, in bytes, which is also the modelled
/// maximum of the `Policy` member (`smithy_query(query="operation",
/// operation="PutResourcePolicy")`: `len 1..20480`).
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

/// The ARN of `name` qualified by `qualifier` — a version label or an alias name.
fn qualified_arn(name: &str, qualifier: &str) -> String {
    format!("{}:{qualifier}", function_arn(name))
}

/// `PutResourcePolicy` on the resource `arn` names, carrying `policy` and optionally a
/// revision precondition.
async fn put_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    policy: &str,
    revision_id: Option<&str>,
) -> Answer {
    let mut body = json!({ "Policy": policy });
    if let Some(revision) = revision_id {
        body["RevisionId"] = json!(revision);
    }
    call(
        router,
        "PUT",
        &format!("{RESOURCE_POLICY_URI}/{arn}"),
        &body,
    )
    .await
}

/// `GetResourcePolicy` on the resource `arn` names.
async fn get_resource_policy(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{RESOURCE_POLICY_URI}/{arn}"),
        &Value::Null,
    )
    .await
}

/// `DeleteResourcePolicy` on the resource `arn` names, optionally carrying a revision
/// precondition (bound to the `RevisionId` query parameter).
async fn delete_resource_policy(
    router: &MultiServiceRouter,
    arn: &str,
    revision_id: Option<&str>,
) -> Answer {
    let query = revision_id.map_or_else(String::new, |revision| format!("?RevisionId={revision}"));
    call(
        router,
        "DELETE",
        &format!("{RESOURCE_POLICY_URI}/{arn}{query}"),
        &Value::Null,
    )
    .await
}

/// Create a function, publishing version 1 when asked.
async fn create_function(router: &MultiServiceRouter, name: &str, publish: bool) {
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

/// Create an alias of `name` pointing at `version`.
async fn create_alias(router: &MultiServiceRouter, name: &str, alias: &str, version: &str) {
    let answer = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &json!({ "Name": alias, "FunctionVersion": version }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateAlias {alias:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Whether `value` is shaped like the model's `RevisionId` (`len 36..36`, pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
fn is_revision_id(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The `Policy` and `RevisionId` of an answer that must carry both.
fn policy_and_revision(case: &str, answer: &Answer) -> (String, String) {
    let policy = answer
        .string("Policy")
        .unwrap_or_else(|| panic!("case {case:?}: no Policy member in {}", answer.text()));
    let revision = answer
        .string("RevisionId")
        .unwrap_or_else(|| panic!("case {case:?}: no RevisionId member in {}", answer.text()));
    (policy, revision)
}

/// A policy document of about `bytes` bytes, padded in the `Sid` of its one statement so
/// the document stays a policy whatever its size.
fn padded_document(bytes: usize) -> String {
    let skeleton = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "pad",
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string();
    let padding = bytes.saturating_sub(skeleton.len());
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": format!("pad{}", "A".repeat(padding)),
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{ACCOUNT}:root") },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string()
}

/// One row of the round-trip table: the resource to address and the document to store on
/// it.
struct DocumentCase {
    name: &'static str,
    /// The qualifier to address the function by, if any.
    qualifier: Option<String>,
    /// The document, as the request carries it.
    policy: String,
}

/// The rows, each differing in the form or size of the document, or in the resource the
/// ARN names.
fn document_cases() -> Vec<DocumentCase> {
    vec![
        // The shape of an ordinary cross-account grant, on the function itself.
        DocumentCase {
            name: "one statement on the function",
            qualifier: None,
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "cross-account-invoke",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
                    "Action": "lambda:InvokeFunction",
                }],
            })
            .to_string(),
        },
        // Several statements, one of them a denial with a condition: the whole document
        // is the caller's, so a policy Lambda would never write for itself round-trips.
        DocumentCase {
            name: "two statements, one of them a conditional deny",
            qualifier: None,
            policy: json!({
                "Version": "2012-10-17",
                "Id": "mixed",
                "Statement": [
                    {
                        "Sid": "allow-org",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "lambda:InvokeFunction",
                        "Condition": { "StringEquals": { "aws:PrincipalOrgID": "o-abcdefghij" } },
                    },
                    {
                        "Sid": "deny-insecure",
                        "Effect": "Deny",
                        "Principal": "*",
                        "Action": "lambda:*",
                        "Condition": { "Bool": { "aws:SecureTransport": "false" } },
                    },
                ],
            })
            .to_string(),
        },
        // The policy language admits a single statement OBJECT as well as a list, and the
        // document is stored as given rather than normalised into a list.
        DocumentCase {
            name: "a single statement object rather than a list",
            qualifier: None,
            policy: json!({
                "Version": "2012-10-17",
                "Statement": {
                    "Sid": "lone",
                    "Effect": "Allow",
                    "Principal": { "Service": "s3.amazonaws.com" },
                    "Action": "lambda:InvokeFunction",
                },
            })
            .to_string(),
        },
        // The unpublished version named explicitly: the ARN pattern admits `$LATEST` as a
        // qualifier, and it names the same policy an unqualified ARN does.
        DocumentCase {
            name: "the unpublished version named as the qualifier",
            qualifier: Some(VERSION_LATEST.to_string()),
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "on-latest",
                    "Effect": "Allow",
                    "Principal": { "AWS": "arn:aws:iam::444455556666:role/invoker" },
                    "Action": "lambda:GetFunction",
                }],
            })
            .to_string(),
        },
        // A published version has its own policy.
        DocumentCase {
            name: "a published version",
            qualifier: Some("1".to_string()),
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "on-version-1",
                    "Effect": "Allow",
                    "Principal": { "Service": "sns.amazonaws.com" },
                    "Action": "lambda:InvokeFunction",
                }],
            })
            .to_string(),
        },
        // So does an alias.
        DocumentCase {
            name: "an alias",
            qualifier: Some("live".to_string()),
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "on-alias",
                    "Effect": "Allow",
                    "Principal": { "Service": "events.amazonaws.com" },
                    "Action": "lambda:InvokeFunction",
                }],
            })
            .to_string(),
        },
        // A document right at the published quota, which is the modelled maximum of the
        // member: the largest policy that may be stored at all.
        DocumentCase {
            name: "a document at the published quota",
            qualifier: None,
            policy: padded_document(MAX_POLICY_SIZE_BYTES),
        },
    ]
}

/// `PutResourcePolicy` stores the document the request carried on the resource the ARN
/// names, and `GetResourcePolicy` reports exactly that document at the revision the write
/// minted.
#[tokio::test]
async fn put_and_get_round_trip_the_document_on_the_resource_the_arn_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, case) in document_cases().into_iter().enumerate() {
        // One function per row, so no row can read another's policy.
        let function = format!("policy-target-{index}");
        create_function(&router, &function, true).await;
        create_alias(&router, &function, "live", "1").await;
        let arn = match &case.qualifier {
            Some(qualifier) => qualified_arn(&function, qualifier),
            None => function_arn(&function),
        };

        let written = put_resource_policy(&router, &arn, &case.policy, None).await;
        assert_eq!(
            written.status,
            200,
            "case {:?}: expected HTTP 200 from PutResourcePolicy, got {} {}",
            case.name,
            written.status,
            written.text()
        );
        let (answered, revision) = policy_and_revision(case.name, &written);
        assert_eq!(
            answered, case.policy,
            "case {:?}: expected the stored document back, got {answered:?}",
            case.name
        );
        assert!(
            is_revision_id(&revision),
            "case {:?}: expected a RevisionId of the modelled form, got {revision:?}",
            case.name
        );

        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: expected HTTP 200 from GetResourcePolicy, got {} {}",
            case.name,
            read.status,
            read.text()
        );
        assert_eq!(
            policy_and_revision(case.name, &read),
            (case.policy.clone(), revision.clone()),
            "case {:?}: expected the stored document and revision, got {}",
            case.name,
            read.text()
        );

        // A report must not change what a later caller sees.
        let again = get_resource_policy(&router, &arn).await;
        assert_eq!(
            policy_and_revision(case.name, &again),
            (case.policy.clone(), revision.clone()),
            "case {:?}: reading the policy changed it: {}",
            case.name,
            again.text()
        );

        // The document is on the stored record, not in per-call scratch state, and only
        // on the qualifier the ARN named.
        let held = engine
            .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
            .lock()
            .functions
            .get(&function)
            .map(|record| {
                record
                    .policies
                    .iter()
                    .map(|(qualifier, policy)| (qualifier.clone(), policy.document.clone()))
                    .collect::<Vec<_>>()
            });
        assert_eq!(
            held,
            Some(vec![(
                case.qualifier
                    .clone()
                    .unwrap_or_else(|| VERSION_LATEST.to_string()),
                Some(case.policy.clone())
            )]),
            "case {:?}: expected the store to hold that one document, got {held:?}",
            case.name
        );
    }
}

/// A second `PutResourcePolicy` replaces the whole policy and mints a new revision, and a
/// `RevisionId` precondition decides whether either writer proceeds.
#[tokio::test]
async fn a_revision_precondition_gates_replacement_and_deletion() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "gated", false).await;
    let arn = function_arn("gated");

    let first = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "first",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string();
    let second = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "second",
            "Effect": "Allow",
            "Principal": { "Service": "sqs.amazonaws.com" },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string();
    // A revision of the modelled form that no policy here is at.
    let stale = "00000000-0000-4000-8000-000000000000";

    let written = put_resource_policy(&router, &arn, &first, None).await;
    let (_, first_revision) = policy_and_revision("the first document", &written);

    // "The revision ID that the existing policy must match for the replacement to
    // proceed": one that does not match refuses the call and changes nothing.
    for (case, answer) in [
        (
            "replacing at a stale revision",
            put_resource_policy(&router, &arn, &second, Some(stale)).await,
        ),
        (
            "deleting at a stale revision",
            delete_resource_policy(&router, &arn, Some(stale)).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            412,
            "case {case:?}: expected HTTP 412, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("PreconditionFailedException"),
            "case {case:?}: expected PreconditionFailedException, got {}",
            answer.error_code()
        );
        let read = get_resource_policy(&router, &arn).await;
        assert_eq!(
            policy_and_revision(case, &read),
            (first.clone(), first_revision.clone()),
            "case {case:?}: the refused call changed the policy, which is now {}",
            read.text()
        );
    }

    // The revision the reader reports is the one that matches, and the replacement is
    // total: the document that was there is gone, at a new revision.
    let replaced = put_resource_policy(&router, &arn, &second, Some(&first_revision)).await;
    assert_eq!(
        replaced.status,
        200,
        "case {:?}: expected HTTP 200, got {} {}",
        "replacing at the current revision",
        replaced.status,
        replaced.text()
    );
    let (answered, second_revision) =
        policy_and_revision("replacing at the current revision", &replaced);
    assert_eq!(
        answered, second,
        "case {:?}: expected the replacing document back, got {answered:?}",
        "replacing at the current revision"
    );
    assert_ne!(
        second_revision, first_revision,
        "case {:?}: expected a new RevisionId after a change, got the earlier one",
        "replacing at the current revision"
    );
    let read = get_resource_policy(&router, &arn).await;
    assert_eq!(
        policy_and_revision("after the replacement", &read),
        (second.clone(), second_revision.clone()),
        "case {:?}: expected the replacing document, got {}",
        "after the replacement",
        read.text()
    );

    // Deleting at the current revision removes the policy, and the resource then has
    // none to report.
    let deleted = delete_resource_policy(&router, &arn, Some(&second_revision)).await;
    assert_eq!(
        deleted.status,
        204,
        "case {:?}: expected HTTP 204, got {} {}",
        "deleting at the current revision",
        deleted.status,
        deleted.text()
    );
    let gone = get_resource_policy(&router, &arn).await;
    assert_eq!(
        gone.status,
        404,
        "case {:?}: expected HTTP 404 once the policy is gone, got {} {}",
        "reading after the deletion",
        gone.status,
        gone.text()
    );
    assert!(
        gone.error_code().contains("ResourceNotFoundException"),
        "case {:?}: expected ResourceNotFoundException, got {}",
        "reading after the deletion",
        gone.error_code()
    );

    // The deletion is idempotent — the resource has no policy either way — but a
    // revision precondition on a resource with no policy can never hold.
    let again = delete_resource_policy(&router, &arn, None).await;
    assert_eq!(
        again.status,
        204,
        "case {:?}: expected HTTP 204, got {} {}",
        "deleting a policy that is already gone",
        again.status,
        again.text()
    );
    let on_gone_policy = delete_resource_policy(&router, &arn, Some(&second_revision)).await;
    assert_eq!(
        on_gone_policy.status,
        412,
        "case {:?}: expected HTTP 412, got {} {}",
        "deleting a policy that is gone at its last revision",
        on_gone_policy.status,
        on_gone_policy.text()
    );
}

/// One row of the refusal table: the call to make, and the status and wire error code it
/// must answer.
struct RefusalCase {
    name: &'static str,
    arn: String,
    policy: String,
    status: u16,
    code: &'static str,
}

/// Every refusal the slice makes, each decided from the request and the state the store
/// holds.
#[tokio::test]
async fn the_resource_policy_operations_refuse_what_they_cannot_store_or_report() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "known", false).await;

    let valid = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ok",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string();

    let refusals = vec![
        // "This is formatted as a JSON string" — and a policy is a JSON object, so
        // neither prose, nor a list, nor a bare number is a policy document.
        RefusalCase {
            name: "a policy that is not JSON at all",
            arn: function_arn("known"),
            policy: "this is not a policy".to_string(),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a policy that is a JSON list",
            arn: function_arn("known"),
            policy: "[{\"Effect\":\"Allow\"}]".to_string(),
            status: 400,
            code: "InvalidParameterValueException",
        },
        RefusalCase {
            name: "a policy that is a bare JSON number",
            arn: function_arn("known"),
            policy: "42".to_string(),
            status: 400,
            code: "InvalidParameterValueException",
        },
        // Past the modelled maximum of the member, which is the published quota: refused
        // as the validation failure it is, before the handler sees it. RECORDED: AWS
        // answers this one `ValidationException` rather than the operation's own
        // `PolicyLengthExceededException`, which it keeps for a policy a GRANT would push
        // past the quota.
        RefusalCase {
            name: "a document past the published quota",
            arn: function_arn("known"),
            policy: padded_document(MAX_POLICY_SIZE_BYTES + 1),
            status: 400,
            code: "ValidationException",
        },
        RefusalCase {
            name: "an ARN naming no function here",
            arn: function_arn("no-such-function"),
            policy: valid.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            name: "an ARN naming a version the function does not have",
            arn: qualified_arn("known", "7"),
            policy: valid.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        // The member's pattern admits a runtime as well as a function, and this store
        // holds no runtime records, so such an ARN names no resource here. Its resource
        // field carries a `/`, which a label escapes, exactly as an SDK sends it.
        RefusalCase {
            name: "an ARN naming a runtime rather than a function",
            arn: format!("arn:aws:lambda:{REGION}:{ACCOUNT}:runtime%2Fpython3_12"),
            policy: valid.clone(),
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];

    for case in &refusals {
        let answer = put_resource_policy(&router, &case.arn, &case.policy, None).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {} from PutResourcePolicy, got {} {}",
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

    // A reader refuses the same resources, and a function nobody has given a policy has
    // none to report.
    for (case, arn) in [
        ("a function with no policy", function_arn("known")),
        (
            "an ARN naming no function here",
            function_arn("no-such-function"),
        ),
        (
            "an ARN naming a runtime rather than a function",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:runtime%2Fpython3_12"),
        ),
    ] {
        let answer = get_resource_policy(&router, &arn).await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected HTTP 404 from GetResourcePolicy, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            answer.error_code()
        );
    }

    // Nothing the refusals asked for was stored.
    let held = engine
        .state::<ComAmazonawsLambdaAwsgirapiserviceState>()
        .lock()
        .functions
        .get("known")
        .map(|record| record.policies.len());
    assert_eq!(
        held,
        Some(0),
        "expected no policy to have been stored by a refused call, got {held:?}"
    );
}

/// One resource has ONE policy: a document stored by `PutResourcePolicy` replaces the
/// statements `AddPermission` added, both readers report it, and a later grant joins it.
#[tokio::test]
async fn one_resource_has_one_policy_whichever_operation_wrote_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "shared", false).await;
    let arn = function_arn("shared");

    // A grant made through `AddPermission` is readable through both readers: it is the
    // same resource's policy.
    let granted = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/shared/policy"),
        &json!({
            "StatementId": "granted",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "case {:?}: expected HTTP 201 from AddPermission, got {} {}",
        "a grant made through AddPermission",
        granted.status,
        granted.text()
    );
    let read = get_resource_policy(&router, &arn).await;
    let (reported, _) = policy_and_revision("a grant made through AddPermission", &read);
    assert!(
        reported.contains("\"granted\""),
        "case {:?}: expected GetResourcePolicy to report the granted statement, got {reported}",
        "a grant made through AddPermission"
    );

    // "If you previously added permissions using the AddPermission operation, the new
    // policy overwrites those permissions."
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "put",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::111122223333:root" },
            "Action": "lambda:InvokeFunction",
        }],
    })
    .to_string();
    let written = put_resource_policy(&router, &arn, &document, None).await;
    assert_eq!(
        written.status,
        200,
        "case {:?}: expected HTTP 200, got {} {}",
        "replacing the granted statements",
        written.status,
        written.text()
    );
    for (case, answer) in [
        (
            "GetResourcePolicy after the replacement",
            get_resource_policy(&router, &arn).await,
        ),
        (
            "GetPolicy after the replacement",
            call(
                &router,
                "GET",
                &format!("{FUNCTIONS_URI}/shared/policy"),
                &Value::Null,
            )
            .await,
        ),
    ] {
        let (reported, _) = policy_and_revision(case, &answer);
        assert_eq!(
            reported, document,
            "case {case:?}: expected both readers to report the stored document, got {reported}"
        );
    }

    // A statement id the stored document already carries is still a conflict, and a new
    // grant joins the document rather than being lost behind it.
    let conflict = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/shared/policy"),
        &json!({
            "StatementId": "put",
            "Action": "lambda:InvokeFunction",
            "Principal": "sns.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        conflict.status,
        409,
        "case {:?}: expected HTTP 409, got {} {}",
        "a statement id the stored document already holds",
        conflict.status,
        conflict.text()
    );
    let added = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/shared/policy"),
        &json!({
            "StatementId": "also-granted",
            "Action": "lambda:InvokeFunction",
            "Principal": "sns.amazonaws.com",
        }),
    )
    .await;
    assert_eq!(
        added.status,
        201,
        "case {:?}: expected HTTP 201, got {} {}",
        "a grant on top of the stored document",
        added.status,
        added.text()
    );
    let merged = get_resource_policy(&router, &arn).await;
    let (reported, _) = policy_and_revision("a grant on top of the stored document", &merged);
    let parsed: Value = serde_json::from_str(&reported).unwrap_or_else(|error| {
        panic!(
            "case {:?}: the policy is not JSON ({error}): {reported}",
            "a grant on top of the stored document"
        )
    });
    let sids: Vec<&str> = parsed
        .pointer("/Statement")
        .and_then(Value::as_array)
        .map(|statements| {
            statements
                .iter()
                .filter_map(|statement| statement.get("Sid").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        sids,
        vec!["put", "also-granted"],
        "case {:?}: expected the stored document to hold both statements, got {reported}",
        "a grant on top of the stored document"
    );
}
