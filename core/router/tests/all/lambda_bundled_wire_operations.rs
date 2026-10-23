// SPDX-License-Identifier: CC0-1.0

//! The ten Lambda operation families a wire replay found refused BEFORE dispatch, each
//! driven over the wire with all four `lambda`-signing services mounted on one router.
//!
//! A wire replay of the whole Lambda corpus answered every lambda-signed request
//! `500 AmbiguousService` with an `application/x-amz-json-1.1` body: four emulated
//! services (the Lambda control plane, Lambda Core, Lambda MicroVMs and Lambda Web) all
//! declare `sigv4Name: lambda`, each claimed on that signing name alone, and the router
//! refuses a request several services claim rather than serving it by an arbitrary one.
//! No Lambda operation models an error named `AmbiguousService`, and the refusal arrives
//! as a retryable 5xx, so an SDK burns its whole retry budget on it.
//!
//! The rule under test: a request signed for a signing name several emulated services
//! share is served by the ONE service whose own model binds the request's method and URI
//! path, and the answer is that operation's own modelled response — its documented status,
//! its body members and its `restJson1` framing — never a routing refusal.
//!
//! `lambda_signing_name_routing.rs` pins the claim itself (which service claims which
//! path, across every API-date segment the four models bind and both wire forms of a
//! path-label ARN). This suite pins the other half, one row per bundled cause of the wire
//! issue: that each of the ten operation families the replay reported — over six API-date
//! path spaces, all four HTTP methods, success and modelled-error paths, every documented
//! `FunctionName` form, and the clock-owed `Pending` -> `Active` transition — reaches its
//! handler and answers from the model while its three peers are mounted beside it.
//!
//! Two of the issue's causes are answered rather than served, and are asserted that way:
//! a synchronous `Invoke` has to RUN the deployment package, which no in-process runtime
//! here can do (its own issue, "Lambda Invoke refuses every real invocation with
//! NotImplemented"), so the row admits the owning service's own answer and only excludes
//! the routing refusal; and `GetPolicy`'s recorded `PublicAccessAllowed` key is not a
//! member of this model's `GetPolicyResponse`
//! (`smithy_query(query="operation", operation="GetPolicy")` gives `Policy` and
//! `RevisionId` only), so nothing asserts it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use smithy_server_runtime::MultiServiceRouter;

use crate::lambda_signing_name_routing::{
    router_with_all_four_on, ACCOUNT, AUTHORIZATION, REGION, ROLE, ZIP_FILE,
};

/// The `FunctionConfiguration` members the replay reported missing on every refused
/// `CreateFunction` / `GetFunctionConfiguration` step, and which the model's own
/// `FunctionConfiguration` output carries
/// (`smithy_query(query="operation", operation="GetFunctionConfiguration")`). Every one of
/// them must be present on a function created with the request below.
///
/// `LastUpdateStatus` is not here: the model reports it only once the update that created
/// the function has finished, so it is asserted on the `Active` read instead.
const CONFIGURATION_MEMBERS: &[&str] = &[
    "Architectures",
    "CodeSha256",
    "CodeSize",
    "Description",
    "EphemeralStorage",
    "FunctionArn",
    "FunctionName",
    "Handler",
    "LastModified",
    "LoggingConfig",
    "MemorySize",
    "PackageType",
    "RevisionId",
    "Role",
    "Runtime",
    "RuntimeVersionConfig",
    "SnapStart",
    "State",
    "Timeout",
    "TracingConfig",
    "Version",
];

/// The `restJson1` response content type these operations answer with
/// (`smithy_query(query="service_info")`: the control plane's only protocol is
/// `aws.protocols#restJson1`).
const REST_JSON_CONTENT_TYPE: &str = "application/json";

/// The content type of the router's own `AmbiguousService` refusal envelope, which the
/// replay also reported on these steps. It is awsJson 1.1 framing and may never appear on
/// an answer from a `restJson1` service.
const REFUSAL_CONTENT_TYPE: &str = "application/x-amz-json-1.1";

/// The two refusals the router itself can answer: several claimants, and none. Neither is
/// a Lambda error code, so neither may be the answer to a request one service owns.
const ROUTING_REFUSALS: &[&str] = &["AmbiguousService", "UnknownService"];

/// What a dispatched request answered. The sibling suite's helper reports only the status,
/// the error code and the body; these rows also assert BOUND RESPONSE HEADERS
/// (`X-Amz-Executed-Version`) and the response framing, so this one keeps the parts.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` when it is empty or not JSON.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// A response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code of an error answer, and the empty string on a success.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or_default()
    }

    /// The body as text, for an assertion message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Dispatch one request, signed for the shared `lambda` signing name.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", content_type);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A JSON request, the shape every control-plane operation here uses. A `Null` body is
/// sent as no body at all, which is what a `GET` or a `DELETE` carries.
async fn json_call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    send(router, method, uri, REST_JSON_CONTENT_TYPE, payload, &[]).await
}

/// Assert one answer against the operation's modelled response: its status, its wire error
/// code (`""` for a success) and its framing. Every failure names the bundled cause, the
/// request and what came back.
fn check(cause: &str, answer: &Answer, expected_status: u16, expected_code: &str) {
    let code = answer.error_code();
    assert!(
        !ROUTING_REFUSALS.contains(&code.as_str()),
        "case {cause:?}: a request exactly one service owns must be served by it, not \
         refused by the router: got HTTP {} {code:?} ({})",
        answer.status,
        answer.text()
    );
    assert_eq!(
        (answer.status, code.as_str()),
        (expected_status, expected_code),
        "case {cause:?}: expected HTTP {expected_status} {expected_code:?} from the one \
         service that owns the path, got {} {code:?} ({})",
        answer.status,
        answer.text()
    );
    let content_type = answer.header("content-type").unwrap_or_default();
    assert_ne!(
        content_type, REFUSAL_CONTENT_TYPE,
        "case {cause:?}: the awsJson 1.1 framing of the router's refusal envelope must \
         never frame a restJson1 answer, got {content_type:?} ({})",
        answer.text()
    );
    if !answer.body.is_empty() {
        assert!(
            content_type.starts_with(REST_JSON_CONTENT_TYPE),
            "case {cause:?}: expected a {REST_JSON_CONTENT_TYPE} body, got \
             {content_type:?} ({})",
            answer.text()
        );
    }
}

/// Assert every member of `CONFIGURATION_MEMBERS` is present on a `FunctionConfiguration`.
fn check_configuration_members(cause: &str, configuration: &Value) {
    for member in CONFIGURATION_MEMBERS {
        assert!(
            !configuration[member].is_null(),
            "case {cause:?}: the FunctionConfiguration must report {member}, got \
             {configuration}"
        );
    }
}

/// The unqualified ARN a function of `name` must carry.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Move every stored function's and capacity provider's activation instant into the past,
/// so the next read materialises the `Pending` -> `Active` transition the clock owes. This
/// is what waiting would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
    for provider in store.capacity_providers.values_mut() {
        provider.active_due = past;
    }
}

/// The `CreateFunction` request the rows below start from: every member the replay's
/// asserted `FunctionConfiguration` reports comes either from it or from a documented
/// default.
fn create_function_request(name: &str) -> Value {
    json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Description": "the bundled wire-replay probe",
        "Code": { "ZipFile": ZIP_FILE },
    })
}

/// Causes 1, 2, 3, 5 and the `DryRun` half of cause 4: the function lifecycle a Lambda
/// client drives first — create, read the configuration back in every documented name
/// form, publish a version, invoke, delete — each answered by the control plane while its
/// three `lambda`-signing peers are mounted beside it.
#[tokio::test]
async fn the_function_lifecycle_causes_are_answered_by_the_owning_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_all_four_on(&engine);
    let name = "bundled-wire-probe";

    // Cause 1: CreateFunction — HTTP 201 with a FunctionConfiguration body.
    let created = json_call(
        &router,
        "POST",
        "/2015-03-31/functions",
        &create_function_request(name),
    )
    .await;
    check("cause 1: CreateFunction", &created, 201, "");
    let created_body = created.json();
    check_configuration_members("cause 1: CreateFunction", &created_body);
    assert_eq!(
        created_body["FunctionArn"],
        json!(function_arn(name)),
        "case \"cause 1: CreateFunction\": expected the ARN {}, got {created_body}",
        function_arn(name)
    );
    assert_eq!(
        created_body["Version"],
        json!("$LATEST"),
        "case \"cause 1: CreateFunction\": an unpublished function is $LATEST, got \
         {created_body}"
    );

    // Cause 2: GetFunctionConfiguration — HTTP 200, the same configuration, and the
    // lifecycle the clock owes once it is due.
    make_activation_due(&engine);
    let configuration_uri = format!("/2015-03-31/functions/{name}/configuration");
    let configuration = json_call(&router, "GET", &configuration_uri, &Value::Null).await;
    check("cause 2: GetFunctionConfiguration", &configuration, 200, "");
    let configuration_body = configuration.json();
    check_configuration_members("cause 2: GetFunctionConfiguration", &configuration_body);
    for (member, expected) in [
        ("State", json!("Active")),
        ("LastUpdateStatus", json!("Successful")),
        ("Description", created_body["Description"].clone()),
        ("CodeSha256", created_body["CodeSha256"].clone()),
        ("CodeSize", created_body["CodeSize"].clone()),
        ("RevisionId", created_body["RevisionId"].clone()),
        ("FunctionArn", created_body["FunctionArn"].clone()),
    ] {
        assert_eq!(
            configuration_body[member], expected,
            "case \"cause 2: GetFunctionConfiguration {member}\": expected {expected}, got \
             {}",
            configuration_body[member]
        );
    }

    // Still cause 2, over the forms the model documents for `FunctionName` ("Function name
    // – my-function … Function ARN … Partial ARN"): each names the same record, and the
    // ARN forms put a `lambda` ARN in the path label the claim rule reads.
    for (form, reference) in [
        ("name only", name.to_string()),
        ("function: prefix", format!("function:{name}")),
        ("partial ARN", format!("{ACCOUNT}:function:{name}")),
        ("function ARN", function_arn(name)),
    ] {
        let case = format!("cause 2: GetFunctionConfiguration by {form}");
        let answer = json_call(
            &router,
            "GET",
            &format!("/2015-03-31/functions/{reference}/configuration"),
            &Value::Null,
        )
        .await;
        check(&case, &answer, 200, "");
        assert_eq!(
            answer.json()["FunctionArn"],
            json!(function_arn(name)),
            "case {case:?}: every documented name form names one record, expected the ARN \
             {}, got {}",
            function_arn(name),
            answer.json()
        );
    }

    // Cause 5: PublishVersion — HTTP 201 with a numeric Version, readable by qualifier.
    let published = json_call(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{name}/versions"),
        &json!({}),
    )
    .await;
    check("cause 5: PublishVersion", &published, 201, "");
    let version = published.json()["Version"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        version.parse::<u64>().is_ok(),
        "case \"cause 5: PublishVersion\": expected a numeric published version, got \
         {version:?} ({})",
        published.json()
    );
    assert_eq!(
        published.json()["FunctionArn"],
        json!(format!("{}:{version}", function_arn(name))),
        "case \"cause 5: PublishVersion\": a published version carries the qualified ARN, \
         got {}",
        published.json()
    );
    let pinned = json_call(
        &router,
        "GET",
        &format!("{configuration_uri}?Qualifier={version}"),
        &Value::Null,
    )
    .await;
    check(
        "cause 5: GetFunctionConfiguration of the version",
        &pinned,
        200,
        "",
    );
    assert_eq!(
        pinned.json()["Version"],
        json!(version),
        "case \"cause 5: GetFunctionConfiguration of the version\": expected version \
         {version}, got {}",
        pinned.json()
    );

    // Cause 4: Invoke. `DryRun` is the one invocation type the model documents as running
    // no function code ("Validate parameter values and verify that the user or role has
    // permission to invoke the function"), and its documented status is 204 with the
    // resolved version in `X-Amz-Executed-Version`.
    for (case, reference, qualifier, expected_version) in [
        (
            "cause 4: Invoke DryRun on $LATEST",
            name.to_string(),
            String::new(),
            "$LATEST".to_string(),
        ),
        (
            "cause 4: Invoke DryRun on the published version",
            name.to_string(),
            format!("?Qualifier={version}"),
            version.clone(),
        ),
        (
            "cause 4: Invoke DryRun by function ARN",
            function_arn(name),
            String::new(),
            "$LATEST".to_string(),
        ),
    ] {
        let answer = send(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{reference}/invocations{qualifier}"),
            "application/octet-stream",
            br#"{"probe":true}"#.to_vec(),
            &[("x-amz-invocation-type", "DryRun")],
        )
        .await;
        check(case, &answer, 204, "");
        assert_eq!(
            answer.header("x-amz-executed-version").as_deref(),
            Some(expected_version.as_str()),
            "case {case:?}: expected X-Amz-Executed-Version {expected_version:?}, got {:?}",
            answer.header("x-amz-executed-version")
        );
    }

    // The rest of cause 4 is the execution itself, which is its own open issue: a
    // synchronous invocation has to RUN the deployment package, and no in-process runtime
    // here can. What this row excludes is the routing refusal — the answer must come from
    // the service that owns the path, which is either its 200 once a runtime can run the
    // package or its `NotImplemented` until then, never a 500 naming four claimants.
    let synchronous = send(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        "application/octet-stream",
        br#"{"probe":true}"#.to_vec(),
        &[("x-amz-invocation-type", "RequestResponse")],
    )
    .await;
    let code = synchronous.error_code();
    assert!(
        !ROUTING_REFUSALS.contains(&code.as_str()) && matches!(synchronous.status, 200 | 501),
        "case \"cause 4: Invoke RequestResponse\": expected the owning service's own \
         answer — 200 with the function's payload once a runtime can run the package, or \
         its 501 NotImplemented until then — got {} {code:?} ({})",
        synchronous.status,
        synchronous.text()
    );

    // Cause 3: DeleteFunction — the empty output's `StatusCode` is bound with
    // `httpResponseCode`, and a deleted function is then not found, so the 204/404
    // distinction a cleanup path needs is observable.
    let deleted = json_call(
        &router,
        "DELETE",
        &format!("/2015-03-31/functions/{name}"),
        &Value::Null,
    )
    .await;
    check("cause 3: DeleteFunction", &deleted, 204, "");
    for (case, method, uri) in [
        (
            "cause 3: DeleteFunction of an already-deleted function",
            "DELETE",
            format!("/2015-03-31/functions/{name}"),
        ),
        (
            "cause 3: GetFunctionConfiguration after the delete",
            "GET",
            configuration_uri.clone(),
        ),
    ] {
        let answer = json_call(&router, method, &uri, &Value::Null).await;
        check(case, &answer, 404, "ResourceNotFoundException");
    }
}

/// Causes 6, 7 and 8: the resource-based policy of a function — write a statement, read
/// the document back, revoke it — each answered by the control plane with its three
/// `lambda`-signing peers mounted beside it.
#[tokio::test]
async fn the_resource_policy_causes_are_answered_by_the_owning_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_all_four_on(&engine);
    let name = "bundled-wire-policy-probe";

    let created = json_call(
        &router,
        "POST",
        "/2015-03-31/functions",
        &create_function_request(name),
    )
    .await;
    check(
        "cause 6: CreateFunction for the policy rows",
        &created,
        201,
        "",
    );
    make_activation_due(&engine);

    let policy_uri = format!("/2015-03-31/functions/{name}/policy");

    // A policy nothing has been granted on yet has no document to report.
    let empty = json_call(&router, "GET", &policy_uri, &Value::Null).await;
    check(
        "cause 7: GetPolicy before any permission",
        &empty,
        404,
        "ResourceNotFoundException",
    );

    // Cause 6: AddPermission — HTTP 201 whose `Statement` member is the statement
    // document that was added. The rows span the principal forms the model's `Principal`
    // admits ("the Amazon Web Services service, Amazon Web Services account, IAM user, or
    // IAM role") and both the unqualified function and a published version.
    let published = json_call(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{name}/versions"),
        &json!({}),
    )
    .await;
    check(
        "cause 6: PublishVersion for the qualified row",
        &published,
        201,
        "",
    );
    let version = published.json()["Version"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let rows = vec![
        (
            "a service principal",
            "service-principal",
            "lambda:InvokeFunction",
            "events.amazonaws.com".to_string(),
            String::new(),
        ),
        (
            "an account principal",
            "account-principal",
            "lambda:InvokeFunction",
            ACCOUNT.to_string(),
            String::new(),
        ),
        (
            "an IAM role principal",
            "role-principal",
            "lambda:GetFunction",
            ROLE.to_string(),
            String::new(),
        ),
        (
            "a statement on a published version",
            "version-principal",
            "lambda:InvokeFunction",
            "events.amazonaws.com".to_string(),
            format!("?Qualifier={version}"),
        ),
    ];

    for (form, statement_id, action, principal, qualifier) in &rows {
        let case = format!("cause 6: AddPermission with {form}");
        let answer = json_call(
            &router,
            "POST",
            &format!("{policy_uri}{qualifier}"),
            &json!({
                "StatementId": statement_id,
                "Action": action,
                "Principal": principal,
            }),
        )
        .await;
        check(&case, &answer, 201, "");
        let statement: Value = answer.json()["Statement"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or(Value::Null);
        assert_eq!(
            statement["Sid"],
            json!(statement_id),
            "case {case:?}: the Statement must be the statement document that was added, \
             got {}",
            answer.json()
        );
        assert_eq!(
            statement["Action"],
            json!(action),
            "case {case:?}: expected the Action that was granted, got {statement}"
        );
        assert!(
            statement.to_string().contains(principal.as_str()),
            "case {case:?}: expected the principal {principal} in the statement, got \
             {statement}"
        );
    }

    // Cause 7: GetPolicy — HTTP 200 with the document and its RevisionId. The qualified
    // policy is its own resource, so the version's statement is not on the function's.
    let policy = json_call(&router, "GET", &policy_uri, &Value::Null).await;
    check("cause 7: GetPolicy", &policy, 200, "");
    let document: Value = policy.json()["Policy"]
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null);
    for (form, statement_id, _, _, qualifier) in &rows {
        let expected = qualifier.is_empty();
        let present = document["Statement"]
            .as_array()
            .is_some_and(|statements| {
                statements
                    .iter()
                    .any(|statement| statement["Sid"] == json!(statement_id))
            });
        assert_eq!(
            present, expected,
            "case \"cause 7: GetPolicy holds {form}\": expected the statement \
             {statement_id} on the function's own policy = {expected}, got {present} \
             ({document})"
        );
    }
    assert!(
        policy.json()["RevisionId"].is_string(),
        "case \"cause 7: GetPolicy\": expected the policy's RevisionId, got {}",
        policy.json()
    );
    let qualified = json_call(
        &router,
        "GET",
        &format!("{policy_uri}?Qualifier={version}"),
        &Value::Null,
    )
    .await;
    check(
        "cause 7: GetPolicy of the published version",
        &qualified,
        200,
        "",
    );

    // Cause 8: RemovePermission — HTTP 204, and the statement is gone from the document.
    for (form, statement_id, _, _, qualifier) in &rows {
        let case = format!("cause 8: RemovePermission of {form}");
        let answer = json_call(
            &router,
            "DELETE",
            &format!("{policy_uri}/{statement_id}{qualifier}"),
            &Value::Null,
        )
        .await;
        check(&case, &answer, 204, "");
    }

    // Every statement revoked leaves no document at all, which is the state the first row
    // of this suite observed.
    let revoked = json_call(&router, "GET", &policy_uri, &Value::Null).await;
    check(
        "cause 8: GetPolicy after every statement is revoked",
        &revoked,
        404,
        "ResourceNotFoundException",
    );
}

/// Causes 9 and 10: the two resources of this endpoint that are not functions — a
/// code-signing configuration (`/2020-04-22/…`) and a capacity provider
/// (`/2025-11-30/…`) — which confirm the refusal preceded dispatch for every operation
/// under the `lambda` prefix rather than for the 2015-03-31 control plane alone.
#[tokio::test]
async fn the_code_signing_and_capacity_provider_causes_are_answered_by_the_owning_service() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_with_all_four_on(&engine);

    // Cause 9: CreateCodeSigningConfig — HTTP 201 with the `CodeSigningConfig`, over both
    // values of the model's `CodeSigningPolicy` enum (`Warn | Enforce`) and the documented
    // default when none is named.
    let profile = format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/probe/1");
    let signing_rows = vec![
        (
            "the documented default policy",
            json!({
                "Description": "bundled wire replay, default policy",
                "AllowedPublishers": { "SigningProfileVersionArns": [profile] },
            }),
            "Warn",
        ),
        (
            "Warn",
            json!({
                "Description": "bundled wire replay, warn",
                "AllowedPublishers": { "SigningProfileVersionArns": [profile] },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": "Warn" },
            }),
            "Warn",
        ),
        (
            "Enforce",
            json!({
                "Description": "bundled wire replay, enforce",
                "AllowedPublishers": { "SigningProfileVersionArns": [profile] },
                "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": "Enforce" },
            }),
            "Enforce",
        ),
    ];

    for (form, request, expected_policy) in &signing_rows {
        let case = format!("cause 9: CreateCodeSigningConfig with {form}");
        let answer = json_call(
            &router,
            "POST",
            "/2020-04-22/code-signing-configs",
            request,
        )
        .await;
        check(&case, &answer, 201, "");
        let config = answer.json()["CodeSigningConfig"].clone();
        for member in [
            "AllowedPublishers",
            "CodeSigningConfigArn",
            "CodeSigningConfigId",
            "CodeSigningPolicies",
            "Description",
            "LastModified",
        ] {
            assert!(
                !config[member].is_null(),
                "case {case:?}: the CodeSigningConfig must report {member}, got {config}"
            );
        }
        assert_eq!(
            config["CodeSigningPolicies"]["UntrustedArtifactOnDeployment"],
            json!(expected_policy),
            "case {case:?}: expected the policy {expected_policy}, got {config}"
        );
        assert_eq!(
            config["Description"], request["Description"],
            "case {case:?}: expected the Description that was asked for, got {config}"
        );

        // Read it back through the implemented reader: the configuration reported is the
        // one that was stored, not one rebuilt from the request.
        let arn = config["CodeSigningConfigArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let fetched = json_call(
            &router,
            "GET",
            &format!("/2020-04-22/code-signing-configs/{arn}"),
            &Value::Null,
        )
        .await;
        check(&format!("{case}, read back"), &fetched, 200, "");
        assert_eq!(
            fetched.json()["CodeSigningConfig"], config,
            "case {case:?}: the configuration read back must be the one created, got {}",
            fetched.json()
        );
    }

    // Cause 10: GetCapacityProvider — HTTP 200 with the `CapacityProvider` envelope, the
    // `Pending` -> `Active` transition the clock owes, and 404 for a provider that does
    // not exist.
    let provider_name = "bundled-wire-provider";
    let create_provider = json!({
        "CapacityProviderName": provider_name,
        "VpcConfig": {
            "SubnetIds": ["subnet-0123456789abcdef0"],
            "SecurityGroupIds": ["sg-0123456789abcdef0"],
        },
        "PermissionsConfig": { "CapacityProviderOperatorRoleArn": ROLE },
        "InstanceRequirements": { "Architectures": ["arm64"] },
        "CapacityProviderScalingConfig": { "ScalingMode": "Auto", "MaxVCpuCount": 64 },
    });
    let created = json_call(
        &router,
        "POST",
        "/2025-11-30/capacity-providers",
        &create_provider,
    )
    .await;
    check("cause 10: CreateCapacityProvider", &created, 202, "");
    let provider_arn =
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{provider_name}");
    assert_eq!(
        created.json()["CapacityProvider"]["CapacityProviderArn"],
        json!(provider_arn),
        "case \"cause 10: CreateCapacityProvider\": expected the ARN {provider_arn}, got {}",
        created.json()
    );
    assert_eq!(
        created.json()["CapacityProvider"]["State"],
        json!("Pending"),
        "case \"cause 10: CreateCapacityProvider\": the activation span is never zero, so \
         a caller acting straight after the create still sees Pending, got {}",
        created.json()
    );

    make_activation_due(&engine);

    for (form, reference) in [
        ("name", provider_name.to_string()),
        ("ARN", provider_arn.clone()),
    ] {
        let case = format!("cause 10: GetCapacityProvider by {form}");
        let answer = json_call(
            &router,
            "GET",
            &format!("/2025-11-30/capacity-providers/{reference}"),
            &Value::Null,
        )
        .await;
        check(&case, &answer, 200, "");
        let provider = answer.json()["CapacityProvider"].clone();
        for member in [
            "CapacityProviderArn",
            "CapacityProviderScalingConfig",
            "InstanceRequirements",
            "LastModified",
            "PermissionsConfig",
            "State",
            "VpcConfig",
        ] {
            assert!(
                !provider[member].is_null(),
                "case {case:?}: the CapacityProvider must report {member}, got {provider}"
            );
        }
        assert_eq!(
            provider["State"],
            json!("Active"),
            "case {case:?}: expected Active once the transition is due, got {provider}"
        );
        for member in [
            "VpcConfig",
            "PermissionsConfig",
            "InstanceRequirements",
            "CapacityProviderScalingConfig",
        ] {
            assert_eq!(
                provider[member], create_provider[member],
                "case {case:?}: expected the {member} that was asked for, got {provider}"
            );
        }
    }

    let unknown = json_call(
        &router,
        "GET",
        "/2025-11-30/capacity-providers/no-such-provider",
        &Value::Null,
    )
    .await;
    check(
        "cause 10: GetCapacityProvider of a provider that does not exist",
        &unknown,
        404,
        "ResourceNotFoundException",
    );
}
